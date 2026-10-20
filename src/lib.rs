//! Library core for the `aim` AppImage manager.
//!
//! This crate exposes the data model and operations so they can be
//! consumed by both the CLI binary and integration tests. The subcommand
//! dispatch lives in [`cli`].
//!
//! SPDX-License-Identifier: MIT OR Apache-2.0

pub mod check;
pub mod cli;
pub mod config;
pub mod desktop;
pub mod doctor;
pub mod download;
pub mod error;
pub mod gc;
pub mod gh;
pub mod hash;
pub mod health;
pub mod identity;
pub mod icon;
pub mod install;
pub mod integrate;
pub mod mime;
pub mod output;
pub mod paths;
pub mod reinstall;
pub mod remove;
pub mod run;
pub mod show;
pub mod staging;
pub mod state;
pub mod upgrade;
pub mod verify;

// Re-exports for convenience at the top of the crate.
pub use error::{Error, Result};
