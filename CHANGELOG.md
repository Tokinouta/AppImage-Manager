# Changelog

All notable changes to `aim` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/) and the project adheres
to [Semantic Versioning](https://semver.org/).

## [0.2.0] — 2026-10-26

### Fixed

- **`gh:` installs now pick release assets by CPU architecture,
  not by size.** Previously, when a release shipped multiple
  AppImages, `aim` downloaded the largest — pure luck on
  multi-arch releases. (Real case: siyuan v3.8.8's `x86_64` and
  `arm64` builds are 0.5% apart; whichever happened to be bigger
  won.) The picker now classifies each asset from its filename
  tokens (`x86_64`/`amd64`/`x64`, `aarch64`/`arm64`,
  `arm`/`armv7`/`armhf`, `i686`/`i386`/`x86`), prefers the
  host-arch match, falls back to unlabeled builds, and errors —
  listing the candidates — when only foreign-arch AppImages
  exist, instead of downloading an unbootable one.

### Changed

- CLI: `aim mime`'s subcommand enum is now `pub` (fixes a
  `private_interfaces` warning); no behaviour change.

## [0.1.1] — 2026-10-20

### Changed

- **License: MIT → `MIT OR Apache-2.0` (dual)**. This is the de-facto
  Rust ecosystem standard (used by serde, tokio, clap, rustc) and
  matches what 9 of 10 direct dependencies of `aim` already advertise.
  Pure-MIT was v0.1.0; the source from v0.1.0 forward is dual-licensed.
  The published v0.1.0 binary and tag remain MIT-only — that source
  is locked-in by the published tag and cannot be retro-licensed.

## [0.1.0] — 2026-10-20

First public release. Manages AppImage install / remove / integrate /
reinstall / upgrade / info / list / mime add|remove|list|default /
doctor / gc / check for AppImages plus their desktop shortcuts.

### Added

- `aim install <id> [--from <path-or-url>]` — atomic install of a local
  AppImage or one downloaded from a GitHub release.
- `aim remove <id>... [--purge]` — drops the state record,
  integration artefacts, and the icon. Default keeps the cached
  AppImage so a future `aim install` is fast; `--purge` deletes it too.
- `aim reinstall <id>` — re-install from a surviving cache (after a
  default `aim remove`). Resolves a bare id against `apps/<id>@*/`.
- `aim integrate <id>... [--no-icon]` — (re)render the `.desktop`,
  install the icon under `$XDG_DATA_HOME/icons/hicolor/`, refresh
  `update-desktop-database`.
- `aim list [--json]` — every install, with optional JSON output.
- `aim info <id> [--json]` — detailed view of one install.
- `aim upgrade <id>... [--check]` — query GitHub for a newer release,
  download, verify sha256, atomically swap.
- `aim mime add|remove|list|default <id> [mime...]` — manage MIME-type
  associations. `default` writes `~/.config/mimeapps.list`.
- `aim doctor`, `aim gc` — health checks; garbage-collect orphan
  downloads.
- `aim check` — verify sha256 of every installed AppImage.
- `--json` on `install` / `remove` / `upgrade` / `integrate`.
- State persistence under `$XDG_CONFIG_HOME/aim/state.toml` with
  `[id@version]` composite keys.
- Composite-identity install: `id@version` derived from the
  AppImage's embedded `.desktop` / filename fallback; trailing-zero
  rule for `1.2.0` vs `1.2`.
- Atomic install via `tempfile_in(parent) + persist`.
- Identity-resilient `copy_into_place` and `remove_any` helpers.
- XDG-respecting layout (`dirs::data_dir`, `dirs::config_dir`).
- `aim --version`, clap-derived `--help`, shell completions.
- `X-Aim-Integrator=true` and `X-Aim-Version=<crate-version>` are
  emitted in every rendered `.desktop` so the file can be
  programmatically identified as aim-generated.
- Type-2 AppImage detection (ELF magic + `AI\x02` at offset 8).
- Strict pedantic lints; `unsafe_code = "forbid"`.
- `~/.local/bin/<id>` symlink created on integrate for command-line
  launches.
- AppImage-without-version test fixture (ELFO + 4 KiB, mode 0755).
- Integration test (`tests/install_local.rs`) that round-trips a
  synthetic Type-2 AppImage through the CLI.

### Fixed

- `.desktop` files now written with mode 0644 (was 0600 from
  `tempfile`'s default); KDE's KIO loader silently rejects files
  that aren't world-readable. New `atomic_write` enforces 0644.
- `Exec=` line always appends a file placeholder (`%F` unless the
  user already supplied `%f/%F/%u/%U`) so double-clicked files
  reach the AppImage.
- `Exec=` no longer wraps the path in `sh -c "..."`. `sh -c CMD ARG`
  makes `ARG` the shell's `$0`, dropping the file path; the AppImage
  launched with empty argv and showed only the start screen.

### Known limitations

- Per-app overrides (`env`, `extra_exec_args`, `comment`, `categories`)
  are NOT preserved across `aim remove` + `aim install` — they lived
  in the deleted state record. Will ship in 0.2.0.
- `aim upgrade` does not auto-upgrade `aim` itself (no self-install
  path yet). A future slice will add an `aim self upgrade` flow.
- KDE cache refresh after `aim integrate` / `aim mime add` is manual
  (`kbuildsycoca6 --noincremental`). Auto-invoke is a polish item.
- `aim` does not auto-suggest MIME types per app. Future: extract
  from AppImage's embedded `.desktop` template (some ship one) or
  a curated lookup table for popular apps.
- `doctor/gc/health.rs` does not yet scan `data_dir/downloads/` for
  orphan `.partial` files. Flagged as polish.
- `aim info --edit` is not implemented.
- `--check-path` self-check (warns when `~/.cargo/bin/aim` is older
  than the workspace `target/release/aim`) is not implemented.

[0.1.0]: https://github.com/Tokinouta/AppImage-Manager/releases/tag/v0.1.0