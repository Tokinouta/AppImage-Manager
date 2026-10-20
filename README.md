# aim — AppImage manager

`aim` is a single-binary CLI for installing, integrating, and
managing AppImages on Linux. It replaces the workflow AppImageLab
onwards expected from `AppImageLauncher` with one tool you can drive
from a shell:

```bash
aim install gh:marktext/marktext   # install from GitHub releases
aim list                            # see what's installed
aim run marktext                    # launch via exec()
aim upgrade marktext                # fetch the latest release
aim doctor                          # cross-check state vs filesystem
```

## Why

`AppImageLauncher` does four things the user actually wanted — and a
handful the user didn't (silent on-demand inotify hooks, fixed
renaming, opaque state, "some buttons didn't seem to work
properly"). `aim` trades that surface area for an auditable shell
state record (`~/.local/share/aim/state.toml`) and explicit,
scriptable subcommands. No background process, no GUI, no surprises.

## Install

```bash
cargo install --path /path/to/appimagemanager
```

Or build a release binary in-place:

```bash
cargo build --release
./target/release/aim --help
```

Requires Rust 1.71+ (edition 2024) and a POSIX shell.

## Quickstart

```bash
# Install from a local file
aim install ~/Downloads/marktext.AppImage

# Install from a URL
aim install https://example.com/marktext.AppImage

# Install from a GitHub repo (uses gh if available, REST otherwise)
aim install gh:marktext/marktext

# List what's installed
aim list

# Get details on one install
aim info marktext

# Launch (signals and exit code flow through `exec`)
aim run marktext

# Upgrade (gh: sources only)
aim check               # see what's outdated
aim upgrade marktext

# Verify the cache hasn't bit-rotted
aim verify marktext

# Sweep orphan files
aim doctor
aim gc --yes
```

## Commands

| Command | Notes |
|---------|-------|
| `aim install <source>` | `source` is a local path, an `http(s)://` URL, or `gh:owner/repo[@ver]`. `--name <ID>` overrides the derived id. `--no-integrate` skips `.desktop` + bin symlink creation. `--force` overwrites an existing install. |
| `aim remove <id...>` | Drops the state record, integration artefacts, and the icon. The cached AppImage stays so a future `aim install` is fast — that's what "cache preserved" refers to. `--purge` also deletes the cached AppImage. |
| `aim reinstall <id>` | Re-install from a surviving cache (after a default `aim remove`). Resolves a bare id against `apps/<id>@*/`. Per-app env / args are NOT preserved (they lived in the deleted state record); the new install starts with fresh defaults. |
| `aim mime add <id> <type>...` | Add MIME types that this install handles. Re-renders `.desktop`, rebuilds the title index. |
| `aim mime remove <id> <type>...` | Remove MIME types from this install's `MimeType=` list. |
| `aim mime list <id>` | Show this install's recorded MIME types. |
| `aim mime default <id> <type>` | Set this install as the OS default for `<type>`. Requires a prior `add`. |
| `aim list [--integrated-only] [--json]` | Tabular view of all installs. |
| `aim info <id> [--json]` | Detailed view (sha256, size, paths, source, install time, per-app overrides). `--edit` is recognised but not yet wired. |
| `aim run <id> [-- <args...>]` | `exec()`s the cached AppImage, forwarding `--` to it. |
| `aim integrate <id>` | Re-renders `.desktop` and bin symlink from current state. Idempotent. |
| `aim unintegrate <id>` | Drops `.desktop` + bin symlink. Cache + state survive. Idempotent. |
| `aim gc [--yes] [--dry-run] [--json]` | Sweeps orphan caches, deskops, bin links, icons. Without `--yes`, prompts; in non-TTY refuses. |
| `aim doctor [--json]` | Read-only diagnostic report. |
| `aim check [--outdated-only] [--json]` | Compares each record's version against the upstream release (gh: sources only). |
| `aim verify <id> [--json]` | Re-hashes the cache against the recorded SHA-256. Exit 1 on mismatch. |
| `aim upgrade <id>` | gh: sources only. Fetches the latest release and atomically replaces the cache. |

## On-disk layout

```
~/.config/aim/                  # config (currently empty; reserved)
~/.local/share/aim/state.toml   # canonical install state
~/.local/share/aim/apps/<id@ver>/AppImage   # cached AppImages
~/.local/share/icons/hicolor/<N>x<N>/apps/aim-<id>.png   # XDG theme path
~/.local/share/aim/downloads/   # partial download tempfiles
~/.local/share/applications/aim-<id>.desktop
~/.local/bin/<id>               # symlink to the cached AppImage
```

All paths are resolvable via XDG (`$XDG_CONFIG_HOME`,
`$XDG_DATA_HOME`); the `~/.local/bin` convention is taken from
the data-dir's parent. Tests use `Layout::with_home(...)` for
isolation.

## Identity

Each install has a key `id@version`. The id comes from the
AppImage's embedded `AppImage.yml` if present (Type-2 AppImages
that carry metadata); otherwise from the asset filename, with
architecture suffixes (`x86_64`, `linux`, `arm64`, …) stripped.
The version comes from `AppImage.yml`'s `image_version` field if
present; otherwise parsed from the filename (`1.0.0`,
`v1.0.0`, `release-1.0.0`, `1.0.0-beta.1` all work). Use
`--name <ID>` to override.

## gh: resolution

`aim install gh:owner/repo` resolves in two steps:

1. The `gh` CLI (`gh release view --repo owner/repo --json
   tagName,assets`) — fast, works for private repos, uses the
   authenticated user's token.
2. The GitHub REST API
   (`api.github.com/repos/{owner}/{repo}/releases/latest`) as a
   fallback. Borrows `gh auth token` opportunistically to bump
   rate limits.

Asset selection is `pick_appimage`: filter by `.appimage`
(case-insensitive); take the single candidate, or the largest of
multiple. Errors with `aim::error::invalid_target` if no
AppImage is in the release.

## Error codes

Each error variant in `aim::error::Error` has a stable string
code (the second column) suitable for grep / scripting:

| Variant | Code |
|---------|------|
| `NotAppImage` | `aim::error::not_appimage` |
| `ExtractFailed` | `aim::error::extract_failed` |
| `InvalidTarget` | `aim::error::invalid_target` |
| `Path` | `aim::error::path` |
| `Io` | `aim::error::io` |
| `StateCorrupt` | `aim::error::state_corrupt` |
| `StateVersion` | `aim::error::state_version` |
| `Toml` | `aim::error::toml` |
| `TomlSer` | `aim::error::toml_ser` |
| `Json` | `aim::error::json` |
| `AlreadyInstalled` | `aim::error::already_installed` |
| `NotInstalled` | `aim::error::not_installed` |
| `Download` | `aim::error::download` |

`miette`'s "fancy" renderer shows the error, the path / id,
and (where useful) a `help:` line.

## Tests

```bash
cargo test          # 81 unit + 4 integration
```

Tests use `Layout::with_home(...)` to keep state isolated in a
tempdir; the integration tests in `tests/install_local.rs`
exercise the full install → state → filesystem flow.

## Development

```
src/
  cli.rs          # clap parser + per-command dispatch
  check.rs        # aim check
  desktop.rs      # .desktop file renderer (single source of truth)
  doctor.rs       # aim doctor (read-only report)
  download.rs     # HTTP downloader with progress bar
  error.rs        # Error enum + miette diagnostics
  gc.rs           # aim gc (with --yes / --dry-run / non-TTY safety)
  gh.rs           # gh:owner/repo resolution (gh CLI + REST)
  hash.rs         # shared streaming SHA-256
  health.rs       # filesystem-vs-state scanner shared by gc + doctor
  icon.rs         # AppImage icon extraction
  identity.rs     # Type-2 AppImage identification (AppImage.yml)
  install.rs      # aim install (the local-path / URL / gh pipeline)
  integrate.rs    # aim integrate / unintegrate
  paths.rs        # Layout (resolve + with_home)
  remove.rs       # aim remove [--purge]
  run.rs          # aim run (exec-replace)
  show.rs         # aim list / info
  staging.rs      # atomic file ops (tempfile, persist, symlink)
  state.rs        # state.toml model + load/save
  upgrade.rs      # aim upgrade
  verify.rs       # aim verify
```

Per-feature commits live on `main`; the design rationale for each
slice (and the bugs fixed during implementation) is documented
in [`CONTEXT.md`](CONTEXT.md).

## Known limitations / polish items

- `aim info --edit` is recognised by clap but not yet wired; the
  intended behaviour is to launch `$EDITOR` against the relevant
  state slice.
- `--json` is wired on `list`, `info`, `gc`, `doctor`, `check`,
  `verify`; not yet on `install`, `remove`, `run`, `integrate`,
  `unintegrate`, `upgrade`.
- The download progress bar only fires from `aim install
  <https://…>` and `aim upgrade`. Other commands are silent.
- `aim gc` does not yet scan `~/.local/share/aim/downloads/` for
  orphaned `.partial` files left behind by crashed installs.
- Bit-rot detection (file-content sha256 ≠ recorded sha256) is in
  `aim verify` only, not in `aim doctor`. Future polish could
  fold it in.
- `aim upgrade` is `gh:`-only. URL installs could be upgraded via
  `ETag` / `If-Modified-Since` in a future slice.
- No file-locking around `state.toml`. Concurrent `aim` processes
  will race; for now the tool is single-user / single-CLI.

## License

Dual-licensed under MIT or Apache 2.0, at your option.