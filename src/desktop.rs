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

    let appimage_path_str = appimage_path.to_string_lossy();
    let extra = record.extra_exec_args.join(" ");
    let exec = if extra.is_empty() {
        appimage_path_str.to_string()
    } else {
        format!("{appimage_path_str} {extra}")
    };

    if record.env.is_empty() {
        let _ = writeln!(out, "Exec={exec}");
    } else {
        // We need the AppImage + extra args to share a single shell
        // command so the env vars lead the line.
        let shell_exec = if extra.is_empty() {
            format!("\"{appimage_path_str}\"")
        } else {
            format!("\"{appimage_path_str}\" {extra}")
        };
        let env_str = record
            .env
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(" ");
        let _ = writeln!(out, "Exec=env {env_str} sh -c {shell_exec}");
    }

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
        assert!(body.contains("Exec=env WAYLAND=1 sh -c \"/cache/foo@1.0.0/AppImage\" --no-sandbox"));
        assert!(body.contains("Icon=aim-foo"));
        assert!(body.contains("StartupWMClass=foo"));
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
        assert!(body.contains("Exec=/cache/foo@1.0.0/AppImage"));
        assert!(!body.contains("Icon="));
        assert!(!body.contains("Comment="));
    }
}
