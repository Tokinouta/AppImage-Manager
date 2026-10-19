//! Tracing + indicatif plumbing.
//!
//! Slice 1 installs run in a single task without progress bars; the
//! `install` and `remove` of large files will benefit from them later.
//! We keep `init()` here so the future command sites only need a
//! single call.

use std::io::IsTerminal;

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter};

/// Initialise tracing + TTY-aware output based on env vars.
///
/// `RUST_LOG=aim=debug` etc. is respected. When stderr is a TTY we
/// use coloured output; otherwise plain.
pub fn init() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("aim=info"));

    let writer = fmt::layer()
        .with_writer(std::io::stderr)
        .with_target(false)
        .with_ansi(std::io::stderr().is_terminal());

    let _ = tracing_subscriber::registry().with(filter).with(writer).try_init();
}

/// Marker trait for output formatting decisions later slices will
/// implement (e.g. table layout for `list`).
#[allow(dead_code)]
pub trait Format {}
