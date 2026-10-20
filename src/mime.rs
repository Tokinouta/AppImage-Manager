//! `aim mime` — manage MIME-type associations for an installed
//! AppImage.
//!
//! Three operations:
//!
//! - `add` — record that this install handles a set of MIME types.
//!   Updates the state record, re-renders the `.desktop` file so
//!   the `MimeType=` line is current, then runs
//!   `update-desktop-database` so the OS knows about it.
//! - `remove` — the opposite. The `.desktop` is regenerated and
//!   the index is refreshed.
//! - `default <mime>` — set this install as the OS default for a
//!   MIME type. Writes `~/.config/mimeapps.list` directly. The
//!   install must already `add` the MIME type first (otherwise the
//!   OS rejects the association as undeclared).
//!
//! We deliberately do not shell out to `xdg-mime default` because
//! `xdg-utils` is not always installed and writing the file
//! directly is portable across all XDG-compliant DEs.

use std::path::PathBuf;

use crate::desktop;
use crate::error::{Error, Result};
use crate::paths::Layout;
use crate::state::State;
use crate::staging::{atomic_write, remove_any};

/// Add `types` to the install's MIME-type list. Idempotent:
/// adding a type that's already recorded is a no-op. Re-renders
/// the `.desktop` and rebuilds the title cache so the OS sees the
/// new line immediately.
pub fn add(target: &str, types: &[String], layout: &Layout) -> Result<()> {
    if types.is_empty() {
        return Err(Error::InvalidTarget {
            message: "aim mime add requires at least one MIME type".to_string(),
        });
    }
    let mut state = State::load(&layout.state_file)?;
    let (key, mut record) = state.lookup(target)?;

    let mut added: Vec<String> = Vec::new();
    for t in types {
        if !record.mime_types.iter().any(|m| m == t) {
            record.mime_types.push(t.clone());
            added.push(t.clone());
        }
    }
    if added.is_empty() {
        println!("{key}: all {} MIME types already recorded", types.len());
        return Ok(());
    }

    persist_and_publish(&mut state, &key, record, layout)?;
    println!("{key}: +{} MIME type(s) — {}", added.len(), added.join(", "));
    Ok(())
}

/// Remove `types` from the install's MIME-type list. Idempotent
/// for non-existent entries (silently skipped). Re-renders the
/// `.desktop` and rebuilds the title cache.
pub fn remove(target: &str, types: &[String], layout: &Layout) -> Result<()> {
    if types.is_empty() {
        return Err(Error::InvalidTarget {
            message: "aim mime remove requires at least one MIME type".to_string(),
        });
    }
    let mut state = State::load(&layout.state_file)?;
    let (key, mut record) = state.lookup(target)?;

    let mut removed: Vec<String> = Vec::new();
    for t in types {
        if let Some(pos) = record.mime_types.iter().position(|m| m == t) {
            record.mime_types.remove(pos);
            removed.push(t.clone());
        }
    }
    if removed.is_empty() {
        println!("{key}: none of the requested MIME types were recorded");
        return Ok(());
    }

    persist_and_publish(&mut state, &key, record, layout)?;
    println!("{key}: -{} MIME type(s) — {}", removed.len(), removed.join(", "));
    Ok(())
}

/// Print the recorded MIME types for the install, one per line.
pub fn list(target: &str, layout: &Layout) -> Result<()> {
    let state = State::load(&layout.state_file)?;
    let (key, record) = state.lookup(target)?;
    if record.mime_types.is_empty() {
        println!("{key}: (no MIME types recorded; use `aim mime add {key} <type>...`)");
    } else {
        println!("{key}:");
        for t in &record.mime_types {
            println!("  {t}");
        }
    }
    Ok(())
}

/// Set this install as the OS default for `mime_type`. Writes
/// `~/.config/mimeapps.list` directly (under
/// `[Default Applications]`). Errors if the install hasn't
/// previously `add`-ed this MIME type — the OS rejects
/// undeclared associations and the user would have a
/// not-actually-working default.
pub fn set_default(target: &str, mime_type: &str, layout: &Layout) -> Result<()> {
    let state = State::load(&layout.state_file)?;
    let (key, record) = state.lookup(target)?;
    let desktop_filename = format!("aim-{}.desktop", sanitize_filename(&key));
    if !record.mime_types.iter().any(|m| m == mime_type) {
        return Err(Error::InvalidTarget {
            message: format!(
                "{key} does not advertise '{mime_type}'. \
                 Run `aim mime add {key} {mime_type}` first."
            ),
        });
    }

    let mimeapps = mimeapps_path();
    let mut lines: Vec<String> = if mimeapps.exists() {
        std::fs::read_to_string(&mimeapps)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    } else {
        Vec::new()
    };

    upsert_default(&mut lines, mime_type, &desktop_filename);
    atomic_write(&mimeapps, lines.join("\n").as_bytes())?;
    println!("{key}: default for {mime_type}");
    Ok(())
}

/// Re-render the `.desktop` file (so the `MimeType=` line is
/// current), save the state, and rebuild the title index so
/// `xdg-mime` and file managers see the new MIME types.
fn rebuild_title_cache(layout: &Layout) {
    // Best-effort: if `update-desktop-database` is missing the
    // user just has to run it manually. We don't fail the
    // operation — the on-disk state is correct either way.
    let _ = std::process::Command::new("update-desktop-database")
        .arg(layout.apps_root_dir.join("applications"))
        .status();
}

fn mimeapps_path() -> PathBuf {
    let config = dirs::config_dir().unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join(".config")
    });
    config.join("mimeapps.list")
}

/// Update the in-memory `lines` of a `mimeapps.list` file so
/// `[Default Applications]` contains the (mime, desktop) entry.
/// Always writes the `[Default Applications]` section header
/// if absent; preserves everything else verbatim.
fn upsert_default(lines: &mut Vec<String>, mime: &str, desktop: &str) {
    // Find the [Default Applications] section bounds.
    let (mut start, mut end) = (None::<usize>, lines.len());
    for (i, line) in lines.iter().enumerate() {
        if line.trim() == "[Default Applications]" {
            start = Some(i + 1);
        } else if start.is_some() && line.starts_with('[') && line.trim().ends_with(']') {
            end = i;
            break;
        }
    }
    let start = match start {
        Some(i) => i,
        None => {
            // No section: append a header (with a blank separator
            // if the file already has content).
            if !lines.is_empty() && !lines.last().unwrap().is_empty() {
                lines.push(String::new());
            }
            lines.push("[Default Applications]".to_string());
            lines.push(format!("{mime}={desktop};"));
            return;
        }
    };

    // Within [start, end), drop any existing line for this MIME.
    let prefix = format!("{mime}=");
    let mut kept: Vec<String> = Vec::with_capacity(lines.len());
    for (i, l) in lines.iter().enumerate() {
        if (start..end).contains(&i) && l.starts_with(&prefix) {
            continue;
        }
        kept.push(l.clone());
    }
    *lines = kept;
    let new_end = lines.len().min(end);
    lines.insert(new_end, format!("{mime}={desktop};"));
}

/// Re-render the `.desktop` and persist the state record.
fn persist_and_publish(
    state: &mut State,
    key: &str,
    record: crate::state::AppRecord,
    layout: &Layout,
) -> Result<()> {
    state.apps.insert(key.to_string(), record.clone());
    state.save(&layout.state_file)?;

    if record.integrated {
        let cache = layout.appimage_path(key);
        if cache.is_file() {
            let body = desktop::render(&record, &cache);
            atomic_write(&layout.desktop_file(key), body.as_bytes())?;
        } else {
            // No cache — nothing to integrate. Drop the .desktop
            // so the OS doesn't see a stale `MimeType=`.
            remove_any(&layout.desktop_file(key));
        }
    }
    rebuild_title_cache(layout);
    Ok(())
}

/// Replace filename-hostile characters in a key, so the result
/// is a valid desktop-file basename.
fn sanitize_filename(key: &str) -> String {
    key.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | ' ' | '\0' => '_',
            c if c.is_ascii_control() => '_',
            c => c,
        })
        .collect()
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
        let home = std::env::temp_dir().join(format!("aim-mime-{nanos}"));
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
    fn add_writes_mime_type_to_desktop() {
        let (layout, home) = fresh_layout();
        install_one(&layout, &home, "Foo.AppImage");
        add(
            "Foo",
            &["text/plain".to_string(), "application/x-foo".to_string()],
            &layout,
        )
        .unwrap();
        let desktop = fs::read_to_string(layout.desktop_file("Foo")).unwrap();
        assert!(desktop.contains("MimeType=text/plain;application/x-foo"));
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn add_is_idempotent() {
        let (layout, home) = fresh_layout();
        install_one(&layout, &home, "Foo.AppImage");
        let types = vec!["text/plain".to_string()];
        add("Foo", &types, &layout).unwrap();
        add("Foo", &types, &layout).unwrap();
        let state = State::load(&layout.state_file).unwrap();
        let (_, rec) = state.lookup("Foo").unwrap();
        assert_eq!(rec.mime_types, vec!["text/plain".to_string()]);
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn remove_takes_them_out() {
        let (layout, home) = fresh_layout();
        install_one(&layout, &home, "Foo.AppImage");
        add(
            "Foo",
            &[
                "text/plain".to_string(),
                "application/x-foo".to_string(),
            ],
            &layout,
        )
        .unwrap();
        remove("Foo", &["text/plain".to_string()], &layout).unwrap();
        let state = State::load(&layout.state_file).unwrap();
        let (_, rec) = state.lookup("Foo").unwrap();
        assert_eq!(rec.mime_types, vec!["application/x-foo".to_string()]);
        let desktop = fs::read_to_string(layout.desktop_file("Foo")).unwrap();
        assert!(!desktop.contains("text/plain"));
        assert!(desktop.contains("application/x-foo"));
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn list_prints_recorded_types() {
        let (layout, home) = fresh_layout();
        install_one(&layout, &home, "Foo.AppImage");
        add("Foo", &["text/plain".to_string()], &layout).unwrap();
        list("Foo", &layout).unwrap();
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn set_default_requires_prior_add() {
        let (layout, home) = fresh_layout();
        install_one(&layout, &home, "Foo.AppImage");
        let err = set_default("Foo", "text/plain", &layout).unwrap_err();
        assert_eq!(err.code(), "aim::error::invalid_target");
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn set_default_writes_mimeapps_list() {
        // The `set_default` function reads `dirs::config_dir()`,
        // which honours `$XDG_CONFIG_HOME`. Setting env vars in
        // tests is unsafe-under-`forbid(unsafe_code)`, so we
        // exercise the pure unit (`upsert_default`) directly here
        // and trust the end-to-end manual run for the
        // `dirs::config_dir()` integration.
        let mut lines = vec!["[Other Section]".to_string(), "x=y".to_string()];
        upsert_default(&mut lines, "text/plain", "aim-Foo.desktop");
        assert!(lines.iter().any(|l| l == "[Default Applications]"));
        assert!(lines.iter().any(|l| l == "text/plain=aim-Foo.desktop;"));
    }

    #[test]
    fn upsert_default_appends_when_no_section() {
        let mut lines = Vec::new();
        upsert_default(&mut lines, "text/plain", "foo.desktop");
        assert!(lines.contains(&"[Default Applications]".to_string()));
        assert!(lines.iter().any(|l| l == "text/plain=foo.desktop;"));
    }

    #[test]
    fn upsert_default_replaces_existing_entry() {
        let mut lines = vec![
            "[Default Applications]".to_string(),
            "text/plain=old.desktop;".to_string(),
            "image/png=other.desktop;".to_string(),
        ];
        upsert_default(&mut lines, "text/plain", "new.desktop");
        assert!(!lines.iter().any(|l| l == "text/plain=old.desktop;"));
        assert!(lines.iter().any(|l| l == "text/plain=new.desktop;"));
        // Other entries preserved.
        assert!(lines.iter().any(|l| l == "image/png=other.desktop;"));
    }

    #[test]
    fn sanitize_filename_replaces_hostile_chars() {
        assert_eq!(sanitize_filename("foo@1.0.0"), "foo@1.0.0");
        assert_eq!(sanitize_filename("foo/bar"), "foo_bar");
        assert_eq!(sanitize_filename("foo bar"), "foo_bar");
    }
}