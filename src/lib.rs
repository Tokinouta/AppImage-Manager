//! Library core for the `aim` AppImage manager.
//!
//! This crate exposes the data model and operations so they can be
//! consumed by both the CLI binary and integration tests. The subcommand
//! dispatch lives in [`cli`].

pub mod cli;
pub mod config;
pub mod desktop;
pub mod doctor;
pub mod download;
pub mod error;
pub mod gc;
pub mod gh;
pub mod health;
pub mod identity;
pub mod icon;
pub mod install;
pub mod integrate;
pub mod output;
pub mod paths;
pub mod remove;
pub mod run;
pub mod show;
pub mod staging;
pub mod state;

// Re-exports for convenience at the top of the crate.
pub use error::{Error, Result};
