//! `.desktop` file rendering.
//!
//! The output follows the freedesktop.org Desktop Entry spec. We
//! include enough keys for `aim <id>` to show up in `rofi`/`dmenu`
//! launchers, GNOME search, and KDE menu.

use std::fmt::Write as _;
use std::path::Path;

use crate::state::AppRecord;

/// Render the `.desktop` file contents for an install.
///
/// The `Exec=` line is always `"<absolute-path-to-AppImage>"`, with
/// `extra_exec_args` appended, wrapped in `env <KEY>=<VALUE> sh -c`
/// when `env` is non-empty (so per-app env overrides propagate).
#[must_use]
pub fn render(record: &AppRecord, appimage_path: &Path) -> String {
    let mut out = String::new();
    out.push_str("[Desktop Entry]\n");
    out.push_str("Type=Application\n");
    out.push_str("Version=1.0\n");

    let _ = writeln!(out, "Name={}", record.id);
    if let Some(c) = &record.comment {
        let _ = writeln!(out, "Comment={c}");
    }
    if !record.categories.is_empty() {
        let _ = writeln!(out, "Categories={}", record.categories.join(";"));
    }
    // MimeType= is what `update-desktop-database` reads to build
    // `mimeinfo.cache`, which is the prerequisite for `xdg-mime`
    // and file-manager "Open With" to consider this app as a
    // candidate for a given file type. Empty when the user hasn't
    // run `aim mime add` — that's correct; the app just won't be
    // a candidate for any type.
    if !record.mime_types.is_empty() {
        let _ = writeln!(out, "MimeType={}", record.mime_types.join(";"));
    }

    let appimage_path_str = appimage_path.to_string_lossy();
    let extra = record.extra_exec_args.join(" ");

    // Compute the env that the *rendered* `.desktop` needs. The
    // record's `env` already contains user overrides; we layer
    // `AIM_INTEGRATED` on top so its value is always derived from the
    // `integrated` flag rather than whatever was in state.
    let mut render_env: std::collections::BTreeMap<&str, &str> =
        record.env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    render_env.insert("AIM_INTEGRATED", if record.integrated { "1" } else { "0" });

    let env_str = render_env
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(" ");
    // Always wrap in `env … sh -c "…"`, even if `render_env` is
    // empty, so the rendered Exec stays a single `sh -c` argument.
    let shell_exec = if extra.is_empty() {
        format!("\"{appimage_path_str}\"")
    } else {
        format!("\"{appimage_path_str}\" {extra}")
    };
    let _ = writeln!(out, "Exec=env {env_str} sh -c {shell_exec}");

    // Icon must be the *theme name* without an extension to work with
    // hicolor index lookup.
    if record.icon_path.is_some() {
        let icon_name = format!("aim-{}", record.id);
        let _ = writeln!(out, "Icon={icon_name}");
    }
    out.push_str("Terminal=false\n");
    out.push_str("StartupNotify=true\n");
    out.push_str("StartupWMClass=");
    out.push_str(&record.id);
    out.push('\n');

    // `aim` adds itself as the integrator so `xdg-desktop-menu
    // --info` and similar tooling can show provenance.
    let _ = writeln!(out, "X-Aim-Integrator=true");
    let _ = writeln!(out, "X-Aim-Version={}", env!("CARGO_PKG_VERSION"));

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Source;
    use std::collections::BTreeMap;

    fn rec() -> AppRecord {
        let mut env = BTreeMap::new();
        env.insert("WAYLAND".to_string(), "1".to_string());
        AppRecord {
            id: "foo".into(),
            version: Some("1.0.0".into()),
            sha256: "x".into(),
            size_bytes: 0,
            integrated: false,
            source: Source::Unknown,
            icon_size_px: Some(512),
            icon_path: Some("hicolor/512x512/apps/aim-foo.png".into()),
            env,
            extra_exec_args: vec!["--no-sandbox".into()],
            comment: Some("A foo app".into()),
            categories: vec!["Utility".into()],
            mime_types: vec![
                "text/plain".into(),
                "application/x-foo".into(),
            ],
            installed_at_secs: 0,
        }
    }

    #[test]
    fn renders_required_keys() {
        let r = rec();
        let body = render(&r, Path::new("/cache/foo@1.0.0/AppImage"));
        assert!(body.starts_with("[Desktop Entry]\n"));
        assert!(body.contains("Type=Application"));
        assert!(body.contains("Name=foo"));
        assert!(body.contains("Comment=A foo app"));
        assert!(body.contains("Categories=Utility"));
        assert!(body.contains("MimeType=text/plain;application/x-foo"));
        assert!(body.contains("Exec=env AIM_INTEGRATED=0 WAYLAND=1 sh -c \"/cache/foo@1.0.0/AppImage\" --no-sandbox"));
        assert!(body.contains("Icon=aim-foo"));
        assert!(body.contains("StartupWMClass=foo"));
    }

    #[test]
    fn icon_name_matches_paths_icon_filename() {
        // Lock in the alignment that the bug report broke: the
        // rendered `Icon=` line must match the basename that
        // `Layout::icon_path` produces.
        let r = rec();
        let body = render(&r, Path::new("/cache/foo@1.0.0/AppImage"));
        let layout = crate::paths::Layout::with_home(std::path::PathBuf::from("/tmp"));
        let filename = layout
            .icon_path("foo@1.0.0", 512)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        // `Icon=` line says `Icon=<filename>-stem>` (no extension).
        let stem = filename.trim_end_matches(".png");
        assert!(
            body.contains(&format!("Icon={stem}")),
            "rendered Icon= must match paths::icon_path filename stem ({filename}):\n{body}"
        );
    }

    #[test]
    fn renders_without_env_or_args() {
        let mut r = rec();
        r.env.clear();
        r.extra_exec_args.clear();
        r.comment = None;
        r.categories.clear();
        r.icon_path = None;
        let body = render(&r, Path::new("/cache/foo@1.0.0/AppImage"));
        // Even with no envs and no extra args, we always wrap in
        // `env ... sh -c ...` so the layout of the Exec line stays
        // stable for diffing.
        assert!(body.contains("Exec=env AIM_INTEGRATED=0 sh -c \"/cache/foo@1.0.0/AppImage\""));
        assert!(!body.contains("Icon="));
        assert!(!body.contains("Comment="));
    }

    #[test]
    fn integrated_flag_drives_aim_integrated_env() {
        let mut r = rec();
        r.env.clear();
        r.integrated = true;
        let body = render(&r, Path::new("/cache/foo@1.0.0/AppImage"));
        assert!(body.contains("Exec=env AIM_INTEGRATED=1"));

        r.integrated = false;
        let body = render(&r, Path::new("/cache/foo@1.0.0/AppImage"));
        assert!(body.contains("Exec=env AIM_INTEGRATED=0"));
    }
}
