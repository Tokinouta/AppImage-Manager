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

The other subcommands exist in the parser but `unimplemented!()` —
they land in slices 2–10 in the order agreed at design time.

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
