//! Top-level error and result types.

use std::path::PathBuf;

use miette::Diagnostic;
use thiserror::Error;

/// Result alias used across the crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors that can surface anywhere in `aim`.
///
/// Variants carry enough context that `miette`'s "fancy" renderer can
/// show users *what* failed, *where*, and *what to do next*. Each
/// variant has a stable string code so users can grep for it.
#[derive(Debug, Error, Diagnostic)]
pub enum Error {
    #[error("not an AppImage: {path}")]
    #[diagnostic(
        code(aim::error::not_appimage),
        help("the file does not look like an AppImage (no ELF magic + AI\\x02 marker)")
    )]
    NotAppImage { path: PathBuf },

    #[error("AppImage extraction failed: {message}")]
    #[diagnostic(code(aim::error::extract_failed))]
    ExtractFailed { message: String },

    #[error("invalid install target: {message}")]
    #[diagnostic(code(aim::error::invalid_target))]
    InvalidTarget { message: String },

    #[error("path error on {path}: {source}")]
    #[diagnostic(code(aim::error::path))]
    Path {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("I/O error: {0}")]
    #[diagnostic(code(aim::error::io))]
    Io(#[from] std::io::Error),

    #[error("state file is corrupt: {message}")]
    #[diagnostic(code(aim::error::state_corrupt))]
    StateCorrupt { message: String },

    #[error("state schema version {found} is not supported (need {required})")]
    #[diagnostic(code(aim::error::state_version))]
    StateVersion { found: u64, required: u64 },

    #[error("TOML error: {0}")]
    #[diagnostic(code(aim::error::toml))]
    Toml(#[from] toml::de::Error),

    #[error("TOML serialization error: {0}")]
    #[diagnostic(code(aim::error::toml_ser))]
    TomlSer(#[from] toml::ser::Error),

    #[error("JSON error: {0}")]
    #[diagnostic(code(aim::error::json))]
    Json(#[from] serde_json::Error),

    #[error("already installed: {id}@{version} (run `aim install --force` to overwrite)")]
    #[diagnostic(code(aim::error::already_installed))]
    AlreadyInstalled { id: String, version: String },

    #[error("not installed: {id}")]
    #[diagnostic(code(aim::error::not_installed))]
    NotInstalled { id: String },
}

impl Error {
    /// Quick stable code for tests / logs.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotAppImage { .. } => "aim::error::not_appimage",
            Self::ExtractFailed { .. } => "aim::error::extract_failed",
            Self::InvalidTarget { .. } => "aim::error::invalid_target",
            Self::Path { .. } => "aim::error::path",
            Self::Io(_) => "aim::error::io",
            Self::StateCorrupt { .. } => "aim::error::state_corrupt",
            Self::StateVersion { .. } => "aim::error::state_version",
            Self::Toml(_) => "aim::error::toml",
            Self::TomlSer(_) => "aim::error::toml_ser",
            Self::Json(_) => "aim::error::json",
            Self::AlreadyInstalled { .. } => "aim::error::already_installed",
            Self::NotInstalled { .. } => "aim::error::not_installed",
        }
    }
}
