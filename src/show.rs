//! Read-only display: `aim list`, `aim info`.
//!
//! Both commands consume the on-disk `state.toml` and never mutate
//! the filesystem. `--json` switches the output format from a
//! human-readable table to a JSON document.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::error::Result;
use crate::paths::Layout;
use crate::state::{AppRecord, Source, State};

/// Options accepted by [`list`].
#[derive(Debug, Clone, Default)]
pub struct ListOpts {
    pub integrated_only: bool,
    pub json: bool,
}

/// Options accepted by [`info`].
#[derive(Debug, Clone, Default)]
pub struct InfoOpts {
    pub json: bool,
}

/// Run `aim list`.
pub fn list(opts: ListOpts, layout: &Layout) -> Result<()> {
    let state = State::load(&layout.state_file)?;
    let records: Vec<(&String, &AppRecord)> = state
        .apps
        .iter()
        .filter(|(_, r)| !opts.integrated_only || r.integrated)
        .collect();

    if opts.json {
        let json_records: Vec<&AppRecord> = records.iter().map(|(_, r)| *r).collect();
        let out = serde_json::to_string_pretty(&json_records)?;
        println!("{out}");
        return Ok(());
    }

    if records.is_empty() {
        println!("no AppImages installed yet");
        return Ok(());
    }

    let key_width = records.iter().map(|(k, _)| k.len()).max().unwrap_or(8).max(8);
    let sha_width = 12;

    println!(
        "{:<key_width$}  {:<10}  {:>10}  {:<sha_width$}  {}",
        "KEY", "INTEGRATED", "SIZE", "SHA256", "INSTALLED"
    );
    for (k, r) in &records {
        let integ = if r.integrated { "yes" } else { "no" };
        let size = human_size(r.size_bytes);
        let sha = &r.sha256[..sha_width.min(r.sha256.len())];
        let installed = format_timestamp(r.installed_at_secs);
        println!(
            "{:<key_width$}  {:<10}  {:>10}  {:<sha_width$}  {}",
            k, integ, size, sha, installed
        );
    }
    Ok(())
}

/// Run `aim info <id>`.
pub fn info(target: &str, opts: InfoOpts, layout: &Layout) -> Result<()> {
    let state = State::load(&layout.state_file)?;
    let (key, record) = state.lookup(target)?;

    if opts.json {
        #[derive(Serialize)]
        struct Envelope<'a> {
            key: &'a str,
            record: &'a AppRecord,
        }
        let env = Envelope { key: &key, record: &record };
        println!("{}", serde_json::to_string_pretty(&env)?);
        return Ok(());
    }

    print_human(&key, &record, layout);
    Ok(())
}

fn print_human(key: &str, r: &AppRecord, layout: &Layout) {
    let version_disp = r.version.as_deref().unwrap_or("(unknown)");
    println!("{}: {}", key, r.id);
    println!("  version:        {version_disp}");
    println!("  sha256:         {}", short_sha(&r.sha256));
    println!("  size:           {} ({} bytes)", human_size(r.size_bytes), r.size_bytes);
    println!("  integrated:     {}", if r.integrated { "yes" } else { "no" });
    let cache = layout.appimage_path(key);
    let desktop = layout.desktop_file(key);
    let bin_link = layout.bin_link(key);
    println!("  cached at:      {}", cache.display());
    println!(
        "  .desktop:       {}{}",
        desktop.display(),
        if desktop.exists() { "" } else { "  (absent)" }
    );
    println!(
        "  bin link:       {}{}",
        bin_link.display(),
        if bin_link.is_symlink() { "" } else { "  (absent)" }
    );
    if let Some(icon_rel) = &r.icon_path {
        println!("  icon:           <data_dir>/{icon_rel}");
    } else {
        println!("  icon:           (none)");
    }
    println!("  source:         {}", format_source(&r.source));
    println!("  installed at:   {}", format_timestamp(r.installed_at_secs));

    let mut printed_section = false;
    if !r.env.is_empty() {
        printed_section = true;
        println!("  per-app env:");
        for (k, v) in &r.env {
            println!("    {k}={v}");
        }
    }
    if !r.extra_exec_args.is_empty() {
        printed_section = true;
        println!("  extra exec args: {}", r.extra_exec_args.join(" "));
    }
    if let Some(c) = &r.comment {
        printed_section = true;
        println!("  comment:        {c}");
    }
    if !r.categories.is_empty() {
        printed_section = true;
        println!("  categories:     {}", r.categories.join(";"));
    }
    if !printed_section {
        println!("  overrides:      (none; edit via `aim info <id> --edit`)");
    }
}

fn format_source(s: &Source) -> String {
    match s {
        Source::Local { path } => format!("local ({path})"),
        Source::Url { url } => format!("url ({url})"),
        Source::Gh { owner, repo } => format!("gh:{owner}/{repo}"),
        Source::Unknown => "unknown".to_string(),
    }
}

fn short_sha(hex: &str) -> String {
    // First 12 + last 4 for "fingerprintable but compact".
    if hex.len() < 16 {
        return hex.to_string();
    }
    format!("{}…{}", &hex[..12], &hex[hex.len() - 4..])
}

fn human_size(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;
    if bytes >= GIB {
        format!("{:.1} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
}

fn format_timestamp(secs: i64) -> String {
    let now =
        SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let delta = now - secs;
    let relative = if delta < 0 {
        format!("in {} seconds", -delta)
    } else if delta < 60 {
        format!("{delta} seconds ago")
    } else if delta < 3600 {
        format!("{} minutes ago", delta / 60)
    } else if delta < 86_400 {
        format!("{} hours ago", delta / 3600)
    } else {
        format!("{} days ago", delta / 86_400)
    };
    format!("{}  ({})", format_iso8601(secs), relative)
}

fn format_iso8601(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let h = rem / 3600;
    let m = (rem % 3600) / 60;
    let s = rem % 60;
    let (y, mo, d) = epoch_days_to_ymd(days);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{m:02}:{s:02} UTC")
}

/// Civil-from-days algorithm (Howard Hinnant). Returns `(year, month, day)`.
fn epoch_days_to_ymd(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::{self, InstallRequest};
    use crate::state::Source;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fresh_layout() -> (Layout, std::path::PathBuf) {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let home = std::env::temp_dir().join(format!("aim-show-{nanos}"));
        fs::create_dir_all(&home).unwrap();
        let layout = Layout::with_home(home.clone());
        (layout, home)
    }

    fn fake_appimage(path: &Path) {
        let mut bytes = vec![0x7f, b'E', b'L', b'F', 0x02, 0x01, 0x01, 0x00, b'A', b'I', 0x02, 0x00];
        bytes.resize(4096, 0);
        fs::write(path, &bytes).unwrap();
        let mut perm = fs::metadata(path).unwrap().permissions();
        perm.set_mode(0o755);
        fs::set_permissions(path, perm).unwrap();
    }

    fn install_one(layout: &Layout, dir: &Path, name: &str) {
        let src = dir.join(name);
        fake_appimage(&src);
        let path_str = src.to_string_lossy().into_owned();
        install::run(
            InstallRequest {
                source: src,
                source_kind: Source::Local { path: path_str },
                name_override: None,
                no_integrate: false,
                force: false,
            },
            &layout,
        )
        .unwrap();
    }

    #[test]
    fn list_empty_state() {
        let (layout, home) = fresh_layout();
        list(ListOpts::default(), &layout).unwrap();
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn list_single_record() {
        let (layout, home) = fresh_layout();
        install_one(&layout, &home, "Foo.AppImage");
        list(ListOpts::default(), &layout).unwrap();
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn list_integrated_only_filters() {
        let (layout, home) = fresh_layout();
        install_one(&layout, &home, "First.AppImage");
        let src = home.join("Second.AppImage");
        fake_appimage(&src);
        let path_str = src.to_string_lossy().into_owned();
        install::run(
            InstallRequest {
                source: src.clone(),
                source_kind: Source::Local { path: path_str },
                name_override: None,
                no_integrate: true,
                force: false,
            },
            &layout,
        )
        .unwrap();
        // Both installed; only First is integrated.
        let state = State::load(&layout.state_file).unwrap();
        let count_all = state.apps.len();
        let count_int = state.apps.values().filter(|r| r.integrated).count();
        assert_eq!(count_all, 2);
        assert_eq!(count_int, 1);
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn info_known_target_succeeds() {
        let (layout, home) = fresh_layout();
        install_one(&layout, &home, "Foo.AppImage");
        info("Foo", InfoOpts::default(), &layout).unwrap();
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn info_unknown_target_errors() {
        let (layout, home) = fresh_layout();
        let err = info("nope", InfoOpts::default(), &layout).unwrap_err();
        assert_eq!(err.code(), "aim::error::not_installed");
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn info_json_has_envelope() {
        let (layout, home) = fresh_layout();
        install_one(&layout, &home, "Foo.AppImage");
        let state = State::load(&layout.state_file).unwrap();
        let (key, rec) = state.lookup("Foo").unwrap();
        #[derive(Serialize)]
        struct Envelope<'a> {
            key: &'a str,
            record: &'a AppRecord,
        }
        let env = Envelope { key: &key, record: &rec };
        let json = serde_json::to_string(&env).unwrap();
        assert!(json.contains("\"key\""));
        assert!(json.contains("\"id\":\"Foo\""));
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn human_size_smoke() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1024), "1.0 KiB");
        assert_eq!(human_size(1024 * 1024), "1.0 MiB");
        assert_eq!(human_size(1024 * 1024 * 1024), "1.0 GiB");
        assert_eq!(human_size(113_210_032), "108.0 MiB");
    }

    #[test]
    fn short_sha_short_input() {
        assert_eq!(short_sha("abcd"), "abcd");
    }

    #[test]
    fn short_sha_long_input() {
        assert_eq!(
            short_sha("2efc8ac22a6dd99b4279bd8f0f6e5e69834b3a68c40c51c1164ded2a03ba9145"),
            "2efc8ac22a6d…9145"
        );
    }

    #[test]
    fn epoch_dates_well_known() {
        assert_eq!(epoch_days_to_ymd(0), (1970, 1, 1));
        // 2000-01-01 is day 10957 from 1970-01-01 (30*365 + 7 leap
        // years between 1970 and 2000). 1999-12-31 is one day earlier.
        assert_eq!(epoch_days_to_ymd(10_956), (1999, 12, 31));
        assert_eq!(epoch_days_to_ymd(10_957), (2000, 1, 1));
    }
}
