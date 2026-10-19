# `aim` — context and spec

A Rust CLI for managing AppImages on Linux. Built because
[AppImageLauncher](https://github.com/TheAssassin/AppImageLauncher) on this
host was unreliable.

## What works in slice 1

- `aim install <local-path>` — copy a local AppImage into the managed
  cache, optionally generate a `.desktop` file and bin symlink, hash
  it, persist to `state.toml`.
- All disk side effects are atomic (tempfile + rename).
- Per-app DirIcon extraction (full `--appimage-extract`, pick the
  biggest PNG, copy into `hicolor/512x512/apps/aim-<id>.png`).
- Composite identity `[id@version]`, with `--name` override.
- Already-installed collision errors unless `--force`.

## What works in slice 2

- `aim integrate <id>` — re-render `.desktop` from current state and
  drop the bin symlink. Idempotent.
- `aim unintegrate <id>` — remove `.desktop` and bin symlink; cache
  + state survive. Idempotent.
- `state::State::lookup(target)` accepts both `id@version` and bare
  `id` (errors if bare-id matches multiple records).
- `desktop::render` is the single source of truth for the
  `.desktop` format. It derives `AIM_INTEGRATED` from
  `record.integrated`, so callers don't have to keep it in
  `record.env`. Slice 1 had two writes (render, then mutate env)
  racing and the env got out of sync — fixed.

## What works in slice 3

- `aim list` — table with KEY / INTEGRATED / SIZE / SHA256 /
  INSTALLED columns. Empty state prints "no AppImages installed yet".
  `--integrated-only` filters to integrated rows.
- `aim info <id>` — full record dump: version, sha (compact
  `2e255…30b7` form), size (human-readable + bytes), integration
  flag, cache path, `.desktop` path, bin link, icon path, source,
  install timestamp + relative time, per-app env/exec-args/comment/
  categories if any.
- `--json` on both: structured output for piping into `jq`.
- Bug fixed: `Layout::with_home` now also redirects
  `$XDG_DATA_HOME/applications/` for `.desktop` files. Slice 1
  tests were silently passing because `desktop_file()` reached into
  the real `$HOME` instead of the test tempdir.

### Timestamp formatting

We compute timestamps ourselves (no `chrono` dep) with Howard
Hinnant's `days_from_civil` algorithm. Tests cover boundary years
(1970, 2000).

## What works in slice 4

- `aim remove <id...>` — drops the state record and removes the
  `.desktop` file and bin symlink. The cached AppImage **stays on
  disk** as an orphan (per spec; "keep for traceability").
- `aim remove <id...> --purge` — same, plus deletes the cache dir
  (`apps/<id@version>/`) and the icon. True uninstall.
- Multi-target removal in one invocation (`aim remove foo bar baz`).
- The first lookup miss aborts the whole run (no partial cleanup);
  this is the obvious per-spec behaviour, easy to make `--keep-going`
  in a future polish slice.
- `aim gc` will sweep the orphaned caches in slice 6; for now they
  sit on disk until `--purge` is used in the same invocation, or
  until the user `rm -rf`s them manually.

## What works in slice 5

- `aim run <id> [-- <args...>]` — launches the cached AppImage,
  forwarding everything after `--` to it. The CLI uses Unix
  `exec()` so the current process *becomes* the AppImage: signals
  (Ctrl-C, SIGTERM) propagate naturally, the exit code of the
  child is preserved bit-for-bit.
- Library API: `aim::run::run(target, args, layout) -> i32` does
  `fork + wait` instead of `exec`, returning the exit code so unit
  tests don't replace themselves.
- Lookup miss: `aim::error::not_installed`. Cache file vanished
  under the state record: `aim::error::invalid_target` (with a
  message pointing at the missing path).

## Storage layout

```
~/.config/aim/config.toml            ← system defaults (slice 1: empty)
~/.local/share/aim/
├── state.toml                       ← single source of truth
├── apps/<id@version>/AppImage       ← cached AppImage (mode 0755)
└── icons/hicolor/<N>x<N>/apps/aim-<id>.png
~/.local/share/applications/aim-<id>.desktop
~/.local/bin/<id>                    ← bin symlink, only when integrated
```

## Identity

- Type-2 AppImages: read `AppImage.yml` via `--appimage-extract`
  for the `id` and `image_version`.
- Fall back to filename heuristics: `Foo-1.2.3-x86_64.AppImage` →
  id=`Foo`, version=`1.2.3`.
- Override with `--name foo`.
- Use `--name` for the rare case where the version really is
  `1.2.3-rc1` (we don't auto-attempt prerelease tokens to avoid
  confusing them with arch suffixes).

## Slice lessons

1. **`std::fs::copy` does not preserve the executable bit.** The
   cached AppImage needs explicit `chmod 0755` or `--appimage-extract`
   in subsequent invocations ENOENTs.
2. **`std::fs::rename` is a move, not a copy.** The first naive
   install used `rename`, which silently consumed the user's source
   file. We now copy via `tempfile_in` + `persist`.
3. **`--appimage-extract <pattern>` only materialises top-level
   symlinks.** Passing `--appimage-extract '*.png'` produces
   `squashfs-root/foo.png` as a symlink into `squashfs-root/usr/...`
   without ever copying the real file. The fix is full extraction;
   pattern-limited is left as a perf option for later.
4. **`miette::fancy` formats errors with codes (`aim::error::*`).
   Useful when something breaks and a user wants to grep logs.

## What slice 1 does NOT do

- URL installs (`gh:owner/repo` or `https://...`) — slice 7 + 8.
- `aim remove`, `aim list`, `aim info` — slices 3 + 4.
- `aim integrate` / `aim unintegrate` — slice 2.
- `aim run`, `aim gc`, `aim check`, `aim doctor` — slices 5/6/9.
- `aim verify`, `aim upgrade` — slice 10.
- JSON output, progress bars (`indicatif`), verbosity flags beyond
  a single `-v` shell-out — slice 5+ polish.

## Test fixtures

`tests/install_local.rs` constructs a synthetic Type-2 AppImage
(ELF magic + `AI\x02` at offset 8 + 4 KiB of zeros) in a tempdir,
so unit tests don't need a network fetch. The CLI was also
verified against `/tmp/marktext.AppImage` (downloaded from GitHub,
113 MB).
