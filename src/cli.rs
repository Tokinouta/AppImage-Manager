//! CLI dispatch.
//!
//! `clap` derives the entire parser. Each subcommand's `run` lives
//! alongside the parser variant — the parser file should stay thin
//! and just route to ops in other modules. Slice 1 routes only
//! `install`.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::error::Result;
use crate::install::{self, InstallRequest};
use crate::integrate;
use crate::paths::Layout;
use crate::show::{self, InfoOpts, ListOpts};
use crate::state::Source;

#[derive(Debug, Parser)]
#[command(name = "aim", about = "AppImage manager", version)]
pub struct Cli {
    /// Increase log verbosity (`-v`, `-vv`).
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,

    /// Suppress non-essential output. Sets log level to `warn`.
    #[arg(short, long, global = true, conflicts_with = "verbose")]
    pub quiet: bool,

    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Debug, Subcommand)]
pub enum Cmd {
    /// Install an AppImage from a local path, URL, or `gh:owner/repo`.
    Install(InstallArgs),
    /// Remove one or more installed AppImages by `id@version`.
    Remove(RemoveArgs),
    /// List installed AppImages.
    List(ListArgs),
    /// Show details for a single install.
    Info(InfoArgs),
    /// Run an installed AppImage, optionally passing through args.
    Run(RunArgs),
    /// Create `.desktop` + bin symlink for an already-installed AppImage.
    Integrate(ManageArgs),
    /// Remove `.desktop` + bin symlink (leaves cache + state intact).
    Unintegrate(ManageArgs),
    /// Garbage-collect orphaned install state.
    Gc,
    /// List installed AppImages with upstream-update availability.
    Check,
    /// Read-only diagnostics: scan for filesystem inconsistencies.
    Doctor,
    /// Re-hash and verify a cached AppImage against its recorded hash.
    Verify(VerifyArgs),
    /// Replace an installed AppImage with a newer release.
    Upgrade(UpgradeArgs),
}

#[derive(Debug, Args)]
pub struct InstallArgs {
    /// Source: local path, URL, or `gh:owner/repo`.
    #[arg(required = true)]
    pub source: String,

    /// Don't create `.desktop` or bin symlink. Use `integrate` later.
    #[arg(long)]
    pub no_integrate: bool,

    /// Override the derived id.
    #[arg(long, value_name = "ID")]
    pub name: Option<String>,

    /// Overwrite an existing install with the same id@version.
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct RemoveArgs {
    /// `id@version` of each install. Pass multiple to remove in one go.
    #[arg(required = true)]
    pub targets: Vec<String>,

    /// Also delete the cached AppImage (default: keep for traceability).
    #[arg(long)]
    pub purge: bool,
}

#[derive(Debug, Args)]
pub struct ListArgs {
    /// Restrict to integrated (`.desktop` + bin symlink present) entries.
    #[arg(long)]
    pub integrated_only: bool,

    /// Emit JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct InfoArgs {
    /// The `id@version` to describe.
    #[arg(required = true)]
    pub target: String,

    /// Open `$EDITOR` to edit per-app overrides.
    #[arg(long)]
    pub edit: bool,

    /// Emit JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct RunArgs {
    /// The `id` (or `id@version`) to run.
    #[arg(required = true)]
    pub target: String,

    /// Args to forward to the AppImage.
    #[arg(last = true)]
    pub app_args: Vec<String>,
}

#[derive(Debug, Args)]
pub struct ManageArgs {
    /// The `id@version` to (un)integrate.
    #[arg(required = true)]
    pub target: String,
}

#[derive(Debug, Args)]
pub struct VerifyArgs {
    /// The `id@version` to verify.
    #[arg(required = true)]
    pub target: String,
}

#[derive(Debug, Args)]
pub struct UpgradeArgs {
    /// The `id@version` to upgrade.
    #[arg(required = true)]
    pub target: String,
}

/// Dispatch the parsed command. Each match arm calls the relevant
/// op module. Errors propagate via `?` and are formatted as `miette`
/// reports in `main`.
pub fn run(cli: Cli) -> Result<()> {
    let layout = Layout::resolve();
    match cli.cmd {
        Cmd::Install(args) => cmd_install(args, &layout),
        Cmd::Remove(_) => unimplemented!("remove lands in slice 4"),
        Cmd::List(args) => show::list(
            ListOpts {
                integrated_only: args.integrated_only,
                json: args.json,
            },
            &layout,
        ),
        Cmd::Info(args) => show::info(
            &args.target,
            InfoOpts { json: args.json },
            &layout,
        ),
        Cmd::Run(_) => unimplemented!("run lands in slice 5"),
        Cmd::Integrate(args) => {
            integrate::integrate(&args.target, &layout)?;
            println!("integrated {}", args.target);
            Ok(())
        }
        Cmd::Unintegrate(args) => {
            integrate::unintegrate(&args.target, &layout)?;
            println!("unintegrated {}", args.target);
            Ok(())
        }
        Cmd::Gc => unimplemented!("gc lands in slice 6"),
        Cmd::Check => unimplemented!("check lands in slice 9"),
        Cmd::Doctor => unimplemented!("doctor lands in slice 6"),
        Cmd::Verify(_) => unimplemented!("verify lands in slice 10"),
        Cmd::Upgrade(_) => unimplemented!("upgrade lands in slice 10"),
    }
}

fn cmd_install(args: InstallArgs, layout: &Layout) -> Result<()> {
    // Slice 1 disambiguation: only local paths are accepted.
    if args.source.contains("://") || args.source.starts_with("gh:") {
        eprintln!(
            "slices 1-2 only support local paths; url / gh: support lands in slices 7-8"
        );
        std::process::exit(2);
    }

    let source_path = PathBuf::from(&args.source);
    let req = InstallRequest {
        source: source_path,
        source_kind: Source::Local { path: args.source.clone() },
        name_override: args.name,
        no_integrate: args.no_integrate,
        force: args.force,
    };

    let result = install::run(req, layout)?;
    println!(
        "installed {}@{} (sha256={}) -> {}",
        result.record.id,
        result.record.version.as_deref().unwrap_or("?"),
        &result.record.sha256[..12],
        layout.appimage_path(&result.record.key()).display()
    );
    if result.integrated {
        println!("integrated (.desktop + bin symlink created)");
    }
    if result.icon_extracted {
        println!("icon extracted");
    }
    Ok(())
}
