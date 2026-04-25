# Self-update

TinyBooth checks for newer releases on every launch and surfaces them in the bottom status bar.

## How it works

1. On startup, a background thread queries `https://api.github.com/repos/ophiocus/TinyBoothSoundStudio/releases/latest`.
2. The latest release's `tag_name` is parsed as a 4-part version (`v1.2.3.4` → `(1, 2, 3, 4)`, missing parts default to 0).
3. If the latest version is greater than the running `APP_VERSION` (compiled in from the build's git tag), the version label in the bottom-left corner becomes a clickable button labelled **vX.Y.Z available — click to install**.
4. Clicking downloads the release's `.msi` asset to your temp directory.
5. The MSI is launched via `Start-Process msiexec ... -Verb RunAs` so Windows prompts for elevation.
6. Once `msiexec` exits successfully, TinyBooth quits — Windows takes over and runs the upgrade in place.

## Version label

The bottom-left of the window always shows `vX.Y.Z` of the running build. This label is **clickable**:

- If the app is currently idle, clicking forces a manual update check.
- If a new version was already detected, the label is replaced with the install button.

## What can go wrong

- **Offline at startup** — silent failure; the version label stays as plain text. Click it to retry once you're online.
- **Rate-limit by GitHub** — same. The unauthenticated API allows ~60 requests per hour per IP, which is plenty for one user.
- **MSI install requires elevation** — the elevation prompt is provided by Windows UAC, not TinyBooth. Decline it and the upgrade simply doesn't happen.

## Skipping a version

There is no per-version skip mechanism. If you don't want the upgrade, don't click the button. The check is offered fresh on every launch.

## Manual download

If the in-app updater fails for any reason, every release is also available at:

`https://github.com/ophiocus/TinyBoothSoundStudio/releases`

The `.msi` is the standard Windows installer; it handles uninstallation of the previous version cleanly via WiX `MajorUpgrade`.
