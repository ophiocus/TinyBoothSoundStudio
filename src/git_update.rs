//! Self-update via GitHub releases.
//!
//! Checks the latest release of `APP_GH_REPO`, compares 4-part semver against
//! `APP_VERSION` (set by build.rs from git tag), and, if newer, downloads the
//! first `.msi` asset and launches it elevated through PowerShell.
//!
//! On successful install-spawn, signals back to the UI thread (via the
//! return value of [`render`]) so `app.rs` can call
//! [`eframe::Frame::close`] for a clean shutdown — Drops run, WAV writers
//! flush, configs save. Pre-v0.3.6 this used `process::exit(0)` directly,
//! which corrupted any in-flight WAV the user had open while updating.

use anyhow::{Context, Result};
use eframe::egui;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Minimum gap between background re-checks of the GitHub releases
/// endpoint. The check itself is a single small JSON GET, but we
/// don't want to hammer the API or burn the user's bandwidth.
/// 5 min matches the documented "CI sync window" of the build
/// pipeline — by the time this interval elapses, a tag pushed at
/// the moment the app was opened should have produced an MSI and
/// updated `releases/latest`.
///
/// Added v0.4.23 — fixes the long-standing known issue where the
/// version label could go stale for the entire session because
/// the check only fired once at startup.
pub const RECHECK_INTERVAL: Duration = Duration::from_secs(300);

#[derive(Debug, Clone)]
pub struct UpdateAvailable {
    pub version: String,
    pub url: String,
    /// URL of the release's `.msi.sha256` sidecar, when published.
    pub sha256_url: Option<String>,
}

pub enum UpdateState {
    Idle,
    Checking,
    Available(UpdateAvailable),
    Downloading(mpsc::Receiver<Result<PathBuf>>),
}

fn is_newer(latest: &str, current: &str) -> bool {
    let parse = |s: &str| -> (u32, u32, u32, u32) {
        let mut p = s.split('.');
        let a = p.next().and_then(|n| n.parse().ok()).unwrap_or(0);
        let b = p.next().and_then(|n| n.parse().ok()).unwrap_or(0);
        let c = p.next().and_then(|n| n.parse().ok()).unwrap_or(0);
        let d = p.next().and_then(|n| n.parse().ok()).unwrap_or(0);
        (a, b, c, d)
    };
    parse(latest) > parse(current)
}

/// Fire a background `check_latest_release()` thread iff:
///   • the updater isn't already busy (state is Idle, no in-flight rx)
///   • `force_now` is set OR `last_check_at` is older than
///     [`RECHECK_INTERVAL`] (or has never run).
///
/// Caller updates `last_check_at = Some(Instant::now())` when this
/// returns `true`. Cheap on the rate-limited path — one
/// `Instant::elapsed()` per frame.
///
/// Added v0.4.23 to close the "version label stays stale because the
/// check fires only at startup" gap.
pub fn maybe_spawn_recheck(
    state: &UpdateState,
    rx: &Option<mpsc::Receiver<Option<UpdateAvailable>>>,
    last_check_at: Option<Instant>,
    force_now: bool,
) -> Option<mpsc::Receiver<Option<UpdateAvailable>>> {
    if !matches!(state, UpdateState::Idle) || rx.is_some() {
        return None;
    }
    let should_run = force_now
        || match last_check_at {
            None => true,
            Some(t) => t.elapsed() >= RECHECK_INTERVAL,
        };
    if !should_run {
        return None;
    }
    let (tx, r) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(check_latest_release());
    });
    Some(r)
}

pub fn check_latest_release() -> Option<UpdateAvailable> {
    let ua = format!("{}/{}", crate::APP_NAME, env!("APP_VERSION"));
    let client = reqwest::blocking::Client::builder()
        .user_agent(ua)
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .ok()?;
    let url = format!(
        "https://api.github.com/repos/{}/releases/latest",
        crate::APP_GH_REPO
    );
    let resp: serde_json::Value = client.get(url).send().ok()?.json().ok()?;
    let tag = resp["tag_name"]
        .as_str()?
        .trim_start_matches('v')
        .to_string();
    if !is_newer(&tag, env!("APP_VERSION")) {
        return None;
    }
    let assets = resp["assets"].as_array()?;
    let dl = assets
        .iter()
        .find(|a| a["name"].as_str().unwrap_or("").ends_with(".msi"))?["browser_download_url"]
        .as_str()?
        .to_string();
    // Optional integrity sidecar (published by the release workflow as
    // of v0.4.85). When present the download is refused on mismatch.
    let sha_url = assets
        .iter()
        .find(|a| a["name"].as_str().unwrap_or("").ends_with(".msi.sha256"))
        .and_then(|a| a["browser_download_url"].as_str())
        .map(|s| s.to_string());
    Some(UpdateAvailable {
        version: tag,
        url: dl,
        sha256_url: sha_url,
    })
}

fn download_and_install(url: &str, version: &str, sha256_url: Option<&str>) -> Result<PathBuf> {
    let ua = format!("{}/{}", crate::APP_NAME, env!("APP_VERSION"));
    let client = reqwest::blocking::Client::builder()
        .user_agent(ua)
        .timeout(std::time::Duration::from_secs(180))
        .build()
        .context("building HTTP client")?;
    let bytes = client
        .get(url)
        .send()
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.bytes())
        .context("downloading MSI")?;

    // Size sanity: a real installer is tens of MB. A tiny body is an
    // error page / truncation — never hand it to an elevated msiexec.
    if bytes.len() < 5_000_000 {
        anyhow::bail!(
            "downloaded installer is implausibly small ({} bytes) — refusing to run it",
            bytes.len()
        );
    }

    // Integrity: when the release ships a .sha256 sidecar, verify or
    // refuse (audit finding: the MSI used to run elevated wholly
    // unverified). Releases predating the sidecar skip this check.
    if let Some(sha_url) = sha256_url {
        let expected = client
            .get(sha_url)
            .send()
            .and_then(|r| r.error_for_status())
            .and_then(|r| r.text())
            .context("downloading checksum sidecar")?;
        let expected = expected
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let actual = sha256_hex(&bytes);
        if expected.len() != 64 || actual != expected {
            anyhow::bail!(
                "installer checksum mismatch (expected {expected}, got {actual}) — \
                 refusing to run it"
            );
        }
    }

    // The version lands in a filename and (previously) an interpolated
    // PowerShell string — sanitize to [0-9.] so a hostile tag_name can
    // never smuggle syntax anywhere (audit finding).
    let safe_version: String = version
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let path = std::env::temp_dir().join(format!("{}-{safe_version}.msi", crate::APP_NAME));
    std::fs::write(&path, &bytes).with_context(|| format!("writing MSI to {}", path.display()))?;

    // Elevation via PowerShell as before, but the only interpolated
    // value is the temp path we just built from sanitized parts.
    let msi = path.to_string_lossy();
    std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!(
                "Start-Process msiexec -ArgumentList '/i \"{msi}\" /passive /norestart' -Verb RunAs"
            ),
        ])
        .spawn()
        .context("launching elevated msiexec via PowerShell")?;

    Ok(path)
}

/// Minimal pure-Rust SHA-256 (FIPS 180-4), test-pinned below. No new
/// dependency: the app's crypto needs start and end at hashing one
/// downloaded file.
fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut msg = data.to_vec();
    let bitlen = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bitlen.to_be_bytes());
    for block in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, c) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([c[0], c[1], c[2], c[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
    h.iter().map(|x| format!("{x:08x}")).collect()
}

/// Drive the version-label widget. Returns `true` exactly once, in the
/// frame where an installer launch has succeeded — the caller should
/// respond by closing the eframe window so Drop impls run cleanly.
#[must_use = "the bool indicates the app should close so Drop impls (WAV finalize, Config save) run; ignoring it leaves the user with a stale window after the installer launches"]
pub fn render(
    ui: &mut egui::Ui,
    state: &mut UpdateState,
    error: &mut Option<String>,
    rx: &mut Option<mpsc::Receiver<Option<UpdateAvailable>>>,
) -> bool {
    let mut should_close = false;

    // Drain background check result.
    if let Some(r) = rx.as_ref() {
        if let Ok(result) = r.try_recv() {
            *state = match result {
                Some(av) => UpdateState::Available(av),
                None => UpdateState::Idle,
            };
            *rx = None;
        }
    }
    // Drain download result. On Ok, signal close so the caller runs a
    // clean eframe shutdown (Drops, flush, save). On Err, surface the
    // anyhow chain and return to Idle so the user can retry.
    if let UpdateState::Downloading(r) = state {
        if let Ok(res) = r.try_recv() {
            match res {
                Ok(_) => {
                    should_close = true;
                }
                Err(e) => {
                    *error = Some(format!("Update failed: {e:#}"));
                    *state = UpdateState::Idle;
                }
            }
        }
    }

    let label = format!("v{}", env!("APP_VERSION"));
    let response = ui
        .add(egui::Label::new(label).sense(egui::Sense::click()))
        .on_hover_text(
            "Installed version. Click to re-check GitHub for a newer \
             release — even when one is already known to be available, \
             a fresh click always does the round trip.",
        );
    // v0.4.26 — click ALWAYS forces a fresh round trip, even when
    // state == Available (was: gated on `state == Idle` so clicking
    // the label while the "v0.4.x available — click to install"
    // button was visible did nothing). Still skip when a check or
    // download is in flight — kicking off a second worker mid-call
    // would race on `*rx`.
    let allow_recheck = !matches!(state, UpdateState::Checking | UpdateState::Downloading(_));
    if response.clicked() && allow_recheck {
        // Drop any in-memory "Available" badge so the user sees the
        // round trip happen (and the newer version, if any) rather
        // than the stale badge sticking around.
        *state = UpdateState::Checking;
        let (tx, r) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(check_latest_release());
        });
        *rx = Some(r);
    }

    match state {
        UpdateState::Idle => {
            if let Some(e) = error.as_ref() {
                ui.colored_label(egui::Color32::LIGHT_RED, e);
            }
        }
        UpdateState::Checking => {
            ui.label("checking…");
        }
        UpdateState::Available(av) => {
            let msg = format!("v{} available — click to install", av.version);
            if ui.add(egui::Button::new(msg)).clicked() {
                let (tx, r) = mpsc::channel();
                let url = av.url.clone();
                let ver = av.version.clone();
                let sha = av.sha256_url.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(download_and_install(&url, &ver, sha.as_deref()));
                });
                *state = UpdateState::Downloading(r);
            }
        }
        UpdateState::Downloading(_) => {
            ui.label("downloading…");
        }
    }

    should_close
}

#[cfg(test)]
mod tests {
    use super::is_newer;

    #[test]
    fn three_part_basic() {
        assert!(is_newer("0.1.1", "0.1.0"));
        assert!(is_newer("0.2.0", "0.1.99"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.1.1"));
    }

    #[test]
    fn four_part_subtag() {
        // Missing components default to 0; "0.1.0" parses as (0,1,0,0).
        assert!(is_newer("0.1.0.1", "0.1.0"));
        assert!(is_newer("0.1.0.10", "0.1.0.9"));
        assert!(!is_newer("0.1.0.0", "0.1.0"));
    }

    #[test]
    fn malformed_components_default_to_zero() {
        assert!(!is_newer("garbage", "0.0.1"));
        assert!(is_newer("0.0.1", "garbage"));
    }

    #[test]
    fn empty_strings() {
        assert!(!is_newer("", ""));
        assert!(!is_newer("", "0.0.1"));
        assert!(is_newer("0.0.1", ""));
    }

    #[test]
    fn major_dominates_minor() {
        assert!(is_newer("2.0.0", "1.99.99"));
        assert!(!is_newer("1.99.99", "2.0.0"));
    }
}

#[cfg(test)]
mod sha_tests {
    use super::sha256_hex;

    /// FIPS 180-4 known-answer vectors — the hand-rolled hash must match
    /// or the updater's integrity check would reject every valid MSI.
    #[test]
    fn sha256_known_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        // Cross the one-block boundary (padding path).
        let long = vec![b'a'; 1000];
        assert_eq!(
            sha256_hex(&long),
            "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3"
        );
    }
}
