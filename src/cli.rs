//! CLI dispatch.
//!
//! `clap` derives the entire parser. Each subcommand's `run` lives
//! alongside the parser variant — the parser file should stay thin
//! and just route to ops in other modules. Slice 1 routes only
//! `install`.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::error::Result;
use crate::check;
use crate::doctor;
use crate::download;
use crate::gc;
use crate::gh;
use crate::install::{self, InstallRequest};
use crate::integrate;
use crate::mime;
use crate::paths::Layout;
use crate::reinstall;
use crate::remove;
use crate::run;
use crate::show::{self, InfoOpts, ListOpts};
use crate::state::Source;
use crate::upgrade;
use crate::verify;

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
    Gc(GcArgs),
    /// List installed AppImages with upstream-update availability.
    Check(CheckArgs),
    /// Read-only diagnostics: scan for filesystem inconsistencies.
    Doctor(DoctorArgs),
    /// Re-hash and verify a cached AppImage against its recorded hash.
    Verify(VerifyArgs),
    /// Replace an installed AppImage with a newer release.
    Upgrade(UpgradeArgs),
    /// Re-install an AppImage from its surviving cache (after a default
    /// `aim remove`).
    Reinstall(ReinstallArgs),
    /// Manage MIME-type associations for an installed AppImage.
    Mime(MimeArgs),
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
pub struct GcArgs {
    /// Skip the interactive confirmation prompt.
    #[arg(short, long)]
    pub yes: bool,

    /// Report what would be deleted but touch nothing.
    #[arg(long)]
    pub dry_run: bool,

    /// Emit JSON listing each removable item.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct DoctorArgs {
    /// Emit JSON listing each issue.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct CheckArgs {
    /// Only show records that have an upstream upgrade available.
    #[arg(long)]
    pub outdated_only: bool,

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

    /// Emit JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct UpgradeArgs {
    /// The `id@version` to upgrade.
    #[arg(required = true)]
    pub target: String,
}

#[derive(Debug, Args)]
pub struct ReinstallArgs {
    /// The `id` (or `id@version`) whose cache to reinstall from.
    #[arg(required = true)]
    pub target: String,

    /// Skip rendering the `.desktop` and bin symlink.
    #[arg(long)]
    pub no_integrate: bool,
}

#[derive(Debug, Args)]
pub struct MimeArgs {
    #[command(subcommand)]
    pub action: MimeAction,
}

#[derive(Debug, Subcommand)]
enum MimeAction {
    /// Add MIME types that this install handles.
    Add {
        /// The `id` (or `id@version`).
        target: String,
        /// One or more MIME types (e.g. `application/vnd.openxmlformats-officedocument.spreadsheetml.sheet`).
        #[arg(required = true)]
        mime_types: Vec<String>,
    },
    /// Remove MIME types from this install's `MimeType=` list.
    Remove {
        target: String,
        #[arg(required = true)]
        mime_types: Vec<String>,
    },
    /// List the MIME types this install handles.
    List {
        target: String,
    },
    /// Set this install as the OS default for a MIME type.
    Default {
        target: String,
        /// The MIME type to set as default.
        mime_type: String,
    },
}

/// Dispatch the parsed command. Each match arm calls the relevant
/// op module. Errors propagate via `?` and are formatted as `miette`
/// reports in `main`.
pub fn run(cli: Cli) -> Result<()> {
    let layout = Layout::resolve();
    match cli.cmd {
        Cmd::Install(args) => cmd_install(args, &layout),
        Cmd::Remove(args) => remove::run(&args.targets, args.purge, &layout),
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
        Cmd::Run(args) => run::exec_replace(&args.target, &args.app_args, &layout),
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
        Cmd::Gc(args) => {
            gc::run(
                gc::GcOpts {
                    yes: args.yes,
                    dry_run: args.dry_run,
                    json: args.json,
                },
                &layout,
            )?;
            Ok(())
        }
        Cmd::Check(args) => check::run(
            check::CheckOpts {
                outdated_only: args.outdated_only,
                json: args.json,
            },
            &layout,
        ),
        Cmd::Doctor(args) => doctor::run(
            doctor::DoctorOpts { json: args.json },
            &layout,
        ),
        Cmd::Verify(args) => verify::run(
            &args.target,
            verify::VerifyOpts { json: args.json },
            &layout,
        ),
        Cmd::Upgrade(args) => upgrade::run(&args.target, &layout),
        Cmd::Reinstall(args) => reinstall::run(
            &args.target,
            reinstall::ReinstallOpts { no_integrate: args.no_integrate },
            &layout,
        ),
        Cmd::Mime(args) => match args.action {
            MimeAction::Add { target, mime_types } => mime::add(&target, &mime_types, &layout),
            MimeAction::Remove { target, mime_types } => mime::remove(&target, &mime_types, &layout),
            MimeAction::List { target } => mime::list(&target, &layout),
            MimeAction::Default { target, mime_type } => {
                mime::set_default(&target, &mime_type, &layout)
            }
        },
    }
}

fn cmd_install(args: InstallArgs, layout: &Layout) -> Result<()> {
    // `gh:owner/repo` resolution → download → install.
    let (source_path, source_kind) = if args.source.starts_with("gh:") {
        let gh_ref = gh::parse(&args.source)?;
        let (tag, asset) = gh::resolve_release(&gh_ref)?;
        eprintln!("resolved gh:{} -> {} ({})", gh_ref.repo, asset.name, tag);
        let cache_dir = layout.data_dir.join("downloads");
        // Use the asset's filename as a hint so the install
        // pipeline's identity fallback sees `marktext-linux-X.Y.Z`
        // rather than `aim-dl-randhex`.
        let dl = download::download(&asset.url, &cache_dir, Some(&asset.name))?;
        (
            dl.path,
            Source::Gh {
                owner: gh_ref.owner,
                repo: gh_ref.repo,
            },
        )
    } else if download::is_url(&args.source) {
        // URL install: download to a temp file in the data dir, then
        // hand the local path to the install pipeline. Use the URL's
        // path basename as a filename hint.
        let cache_dir = layout.data_dir.join("downloads");
        let hint = url_path_basename(&args.source);
        let dl = download::download(&args.source, &cache_dir, hint.as_deref())?;
        let url = args.source.clone();
        (dl.path, Source::Url { url })
    } else {
        let p = PathBuf::from(&args.source);
        let path_str = p.to_string_lossy().into_owned();
        (p, Source::Local { path: path_str })
    };

    let req = InstallRequest {
        source: source_path,
        source_kind,
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

/// Pull the last non-empty path component from a URL, used as a
/// filename hint for downloads. Returns `None` if the URL has no
/// usable basename.
fn url_path_basename(url: &str) -> Option<String> {
    // Strip query string and fragment.
    let path = url.split_once('?').map(|(p, _)| p).unwrap_or(url);
    let path = path.split_once('#').map(|(p, _)| p).unwrap_or(path);
    let last = path.rsplit_once('/').map(|(_, b)| b).unwrap_or(path);
    if last.is_empty() {
        None
    } else {
        Some(last.to_string())
    }
}
