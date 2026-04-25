//! In-app manual — Markdown chapters embedded at compile time.
//!
//! Each chapter source lives under `docs/manual/` in the repo. `include_str!`
//! reads the bytes during the build, so the running binary is fully
//! self-contained — no runtime file lookup, no install-path resolution,
//! no chance of a doc-vs-binary version mismatch after a partial upgrade.
//!
//! To add a chapter: drop `docs/manual/NN-slug.md` next to the others,
//! then push a new entry to [`PAGES`]. The order of `PAGES` is the order
//! of the table of contents; categories define the visual grouping.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Welcome,
    Reference,
    Appendix,
}

impl Category {
    pub fn label(self) -> &'static str {
        match self {
            Self::Welcome => "Welcome",
            Self::Reference => "Reference",
            Self::Appendix => "Appendix",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ManualPage {
    /// Stable identifier — used in the `manual_slug` app-state field.
    pub slug: &'static str,
    /// Title shown in the table of contents.
    pub title: &'static str,
    pub category: Category,
    /// Raw markdown content, embedded at build time.
    pub markdown: &'static str,
}

pub const PAGES: &[ManualPage] = &[
    ManualPage {
        slug: "index",
        title: "Welcome",
        category: Category::Welcome,
        markdown: include_str!("../docs/manual/00-index.md"),
    },
    ManualPage {
        slug: "getting-started",
        title: "Getting started",
        category: Category::Welcome,
        markdown: include_str!("../docs/manual/01-getting-started.md"),
    },
    ManualPage {
        slug: "recording",
        title: "Recording",
        category: Category::Reference,
        markdown: include_str!("../docs/manual/02-recording.md"),
    },
    ManualPage {
        slug: "recording-tones",
        title: "Recording tones",
        category: Category::Reference,
        markdown: include_str!("../docs/manual/03-recording-tones.md"),
    },
    ManualPage {
        slug: "admin",
        title: "Editing profiles (Admin)",
        category: Category::Reference,
        markdown: include_str!("../docs/manual/04-admin.md"),
    },
    ManualPage {
        slug: "projects",
        title: "Projects",
        category: Category::Reference,
        markdown: include_str!("../docs/manual/05-projects.md"),
    },
    ManualPage {
        slug: "export",
        title: "Export",
        category: Category::Reference,
        markdown: include_str!("../docs/manual/06-export.md"),
    },
    ManualPage {
        slug: "suno-import",
        title: "Importing Suno stems",
        category: Category::Reference,
        markdown: include_str!("../docs/manual/07-suno-import.md"),
    },
    ManualPage {
        slug: "self-update",
        title: "Self-update",
        category: Category::Reference,
        markdown: include_str!("../docs/manual/08-self-update.md"),
    },
    ManualPage {
        slug: "using-this-manual",
        title: "Using this manual",
        category: Category::Reference,
        markdown: include_str!("../docs/manual/09-using-this-manual.md"),
    },
    ManualPage {
        slug: "troubleshooting",
        title: "Troubleshooting",
        category: Category::Appendix,
        markdown: include_str!("../docs/manual/appendix-a-troubleshooting.md"),
    },
    ManualPage {
        slug: "file-formats",
        title: "File formats",
        category: Category::Appendix,
        markdown: include_str!("../docs/manual/appendix-b-file-formats.md"),
    },
];

pub fn find(slug: &str) -> Option<&'static ManualPage> {
    PAGES.iter().find(|p| p.slug == slug)
}

pub const DEFAULT_SLUG: &str = "index";
