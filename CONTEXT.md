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

## What works in slice 6

- `aim doctor` — read-only cross-check of `state.toml` against
  the filesystem. Detects: MISSING-CACHE (state says file, gone),
  MISSING-DESKTOP / MISSING-BIN-LINK (integrated=true but
  artefact absent), DANGLING-BIN-LINK (symlink → nothing),
  ORPHAN-CACHE / ORPHAN-DESKTOP / ORPHAN-BIN-LINK (file on disk
  with no state record).
- `aim gc [--yes] [--dry-run] [--json]` — sweeps the *recoverable*
  subset of issues: orphans + danglers. Bit-rot detection /
  Missing-Cache recovery is intentionally NOT in gc (those are
  state inconsistencies that need a human; slice 10's `verify` /
  `upgrade` handle bit-rot surgically).
- `--dry-run` shows what would be removed, deletes nothing.
- Without `--yes` in a non-TTY: prints the plan, refuses, suggests
  `--yes`. With TTY: prompts `Proceed? [y/N]`.
- Shared scanner: `aim::health::scan(layout) -> Vec<Issue>` is the
  single source of truth; doctor and gc both consume it.
- `IssueKind` is `serde::Serialize` with
  `rename_all = "SCREAMING-KEBAB-CASE"`, so JSON output matches
  the human-readable labels.

### ORPHAN-CACHE cleanup includes the parent dir

When `IssueKind::OrphanCache` triggers, `Issue::cleanup_paths()`
returns both the AppImage file and its parent `apps/<key>/` dir
so the leftover empty dir doesn't sit on disk.

## What works in slice 7

- `aim install <https://…>` — downloads the AppImage to a temp
  file under `data_dir/downloads/`, then runs the same install
  pipeline as the local-path code path. The state record's `source`
  is `{ kind: "url", url: <orig> }`.
- HTTP via `ureq` 2.x (rustls under the hood, default features
  pick `tls`).
- Progress bar (via `indicative`): bytes/total when
  `Content-Length` is present, spinner otherwise.
- On any I/O error mid-download the partial file is cleaned up by
  the tempdir drop guard.
- New error variant: `aim::error::download`. The `Download { url,
  message }` payload is what users will see when the network is
  down or the server returns 4xx/5xx.
- `gh:owner/repo` is detected at the CLI level and refused with
  "lands in slice 8" so the format is recognised but not yet
  wired up.

### Follow-up: `downloads/` orphan detection

The health scanner (slice 6) doesn't yet look at `data_dir/downloads/`
for orphaned `.partial` files. A failed install can leave one
behind if the OS killed the process between `tempfile_in` and
`into_temp_path()`. Slice 7 marks this; we can fold it into
`aim gc` in a follow-up polish slice, or as part of slice 8.

## What works in slice 8

- `aim install gh:owner/repo` — resolves to the latest release,
  picks the AppImage asset, downloads it, and runs the install
  pipeline. State record uses `Source::Gh { owner, repo }`.
- Parser: `gh:owner/repo`, `gh:owner/repo@version`, and
  `gh:owner/repo.git` (the `.git` suffix is stripped).
- Two resolution paths: the `gh` CLI (preferred; uses the
  authenticated user's token transparently, works for private
  repos) and a REST fallback against
  `api.github.com/repos/{owner}/{repo}/releases/latest`. The
  REST path also opportunistically reads `gh auth token` to bump
  rate limits; this requires no new config from the user.
- Asset selection (`pick_appimage`): filter by `.appimage` (case
  insensitive) extension, take the single candidate, or the
  largest of multiple. Errors if the release has no AppImage.
- `gh` CLI version drift: gh 2.102+ emits the field as `"url"`,
  older versions as `"browserDownloadUrl"`. Both are accepted via
  `serde(alias = …)`.
- `name_hint` plumbing: when slice 7's downloader is given a hint
  (the asset's name for gh:, the URL path's basename for URL
  installs), the temp file is renamed verbatim to the hint so the
  install pipeline's identity fallback sees the real name. Without
  the hint the install id is `aim-dl-XXXXXX`, which is unhelpful.

### UX note: bin link quoting

The bin link for `marktext-linux@0.22.0` is
`~/.local/bin/marktext-linux@0.22.0`. The `@` is a valid filename
character on Linux, but the user has to quote it in the shell.
The cleaner workaround for users who want a pretty name is
`aim install gh:marktext/marktext --name marktext`, which makes
the id and bin link just `marktext`.

## What works in slice 9

- `aim check [--outdated-only] [--json]` — for every state record,
  reports `LOCAL`, `UPSTREAM`, and `STATUS`:
    - `up-to-date` — versions match
    - `upgrade available` — upstream is newer
    - `ahead of upstream` — local is newer (custom build, e.g.)
    - `unknown` — either side failed to parse
    - `not trackable` — source is `Local` / `Url` / `Unknown`
    - `upstream check failed: <why>` — GitHub resolution errored;
      the rest of the report still prints so one bad record
      doesn't kill the run.
- Version parsing: extracts the leading numeric prefix of each
  dot/dash/plus-separated segment, so `1.0.0-beta.1` →
  `[1, 0, 0]` and `2.0.0rc1` → `[2, 0, 0]`. Tolerant of `v`- and
  `release-`-prefixes.
- Version comparison: semver-style, with the rule that trailing
  zeros are equivalent (`1.0 == 1.0.0`).
- Reuses `gh::resolve_release` from slice 8 for the upstream
  fetch; both `gh` CLI and REST paths now correctly propagate the
  release tag.

### Bug found + fixed in slice 9

`gh::resolve_release` was hardcoding the tag as `"latest"` for the
gh CLI path, hiding the real version from `aim check`. The fix was
two parts: thread the actual `tagName` through `try_gh_cli` (the
function now returns `(String, Vec<Asset>)`), and add
`#[serde(rename_all = "camelCase")]` to `GhCliRelease` so the
field is read as `tagName` (gh's output) instead of `tag_name`.

## What works in slice 10

- `aim verify <id> [--json]` — re-hash the cached AppImage against
  the recorded SHA-256. On mismatch prints both hashes and returns
  exit code 1 with `aim::error::invalid_target`. The streaming
  SHA-256 helper was extracted to `crate::hash::hash_file` so
  install, verify, and upgrade all share one implementation.
- `aim upgrade <id>` — for `gh:` installs only. Resolves the
  latest release via `gh::resolve_release`, compares versions,
  downloads the new asset, identifies the new AppImage
  (`AppImage.yml` → filename fallback), then:
    1. Atomic cache replacement (tempfile_in + chmod + persist).
    2. Re-extract the icon (the new AppImage may have a different
       image).
    3. Re-render the `.desktop` and bin symlink under the new key
       when integrated.
    4. Drop the old `.desktop`, bin symlink, and the old
       `apps/<old_key>/` directory.
    5. Update `state.toml`: remove the old record, insert the new
       one with the same `source`, `integrated` flag, and per-app
       overrides (`env`, `extra_exec_args`, `comment`,
       `categories`).
  - On `up-to-date`: prints "already up-to-date (<tag>)", no-op.
  - On `ahead of upstream`: refuses (no downgrade).
  - On unparseable versions: errors with a clear message; does
    not download.
  - On `AlreadyInstalled` collision (new key already in state):
    errors; user must `aim remove <new>` first.

### Edge case: state key vs. version field

The BTreeMap key is `id@version` and the record's `version` field
should match. If they disagree (state corruption), `aim upgrade`
will refuse: e.g. upgrading `marktext-linux@0.22.0` (whose
record says `version = "0.21.0"`) to upstream 0.22.0 will hit
the AlreadyInstalled check because the new key would collide with
the old one. The user can fix this by editing the state, or by
`aim remove` + reinstall.

### End-to-end upgrade verified

1. Faked state: key=`marktext-linux@0.21.0`, version=`0.21.0`.
2. `aim check` → "upgrade available".
3. `aim upgrade marktext-linux@0.21.0`:
   - resolves v0.22.0 from upstream
   - downloads the new asset
   - identifies the new AppImage (id=`marktext-linux`,
     version=`0.22.0`)
   - atomic cache replace
   - re-renders `.desktop` + bin symlink
   - drops old `apps/marktext-linux@0.21.0/`
   - state now has `marktext-linux@0.22.0` with `integrated =
     true` and a fresh `installed_at_secs`
4. `aim verify` → "OK"
5. `aim check` → "up-to-date"

## Bug fix: icon filename mismatch

The `.desktop` file's `Icon=` line was `aim-<id>` (e.g.
`Icon=aim-Readest`), but `paths::icon_path` was building the
filename from the full key, producing `aim-<id@version>.png`
(e.g. `aim-Readest@0.12.16.png`). The two names didn't refer
to the same file, so desktop environments couldn't resolve the
icon for any install where the AppImage was identified by both
id and version — i.e. all of them.

Fixed by stripping the `@version` part in `paths::icon_path`
(via a new `id_for_key` helper), so the file lands at the name
the renderer references. Existing installs need a one-time manual
migration:

```bash
mv ~/.local/share/aim/icons/hicolor/512x512/apps/aim-<id>@<ver>.png \
   ~/.local/share/aim/icons/hicolor/512x512/apps/aim-<id>.png
sed -i 's|aim-<id>@<ver>.png|aim-<id>.png|' ~/.local/share/aim/state.toml
aim integrate <id>@<ver>      # re-render the .desktop
```

A future polish slice could add an `aim doctor` rule that
detects the old-style icon files and migrates them automatically.

Tests: 4 new in paths + desktop locking in the alignment
(id_for_key_strips_version, icon_path_uses_id_only,
icon_path_no_version_uses_full_key, icon_name_matches_paths_icon_filename).
85 unit + 4 integration green total.

## Bug fix: icons in the wrong XDG directory

The first fix only aligned the file *name* — the directory was
still wrong. `Layout::icons_dir` resolved to
`~/.local/share/aim/icons`, but the XDG icon theme spec puts
user-installable icons in `~/.local/share/icons/`. The icon
theme search path (used by hicolor, Adwaita, Breeze, …) only
looks under `~/.local/share/icons/`, so even a correctly-named
file in the wrong directory is invisible to the desktop.

Fix: `Layout::icons_dir` now points to `$XDG_DATA_HOME/icons`
(sibling to our app data dir) in both `resolve()` and
`with_home()`. Existing installs need their icon files moved
from `~/.local/share/aim/icons/...` to
`~/.local/share/icons/...`, then `gtk-update-icon-cache` to
rebuild the theme index.

Tests: same 85 unit + 4 integration still pass; the existing
icon-path tests caught the alignment but not the directory —
worth adding an integration test that asserts the icon ends up
in the XDG theme location.

## Bug fix: missing `icons/` segment in icons_dir

The previous fix set `icons_dir = dirs::data_dir()` (which is
`~/.local/share`), so `icon_path` joined that with
`hicolor/512x512/apps/...` and produced
`~/.local/share/hicolor/512x512/apps/aim-<id>.png` — missing
the `icons/` segment. The correct path is
`~/.local/share/icons/hicolor/512x512/apps/aim-<id>.png`.

Fix: join `dirs::data_dir()` with `"icons"` in `resolve()`, and
`home/.local/share` with `"icons"` in `with_home()`. Added a
new regression test (`icon_path_lands_in_xdg_theme_directory`)
that asserts the icon path contains `/share/icons/hicolor/`
and does NOT contain `/share/aim/`. 86 unit tests, all green.

Existing installs needed their icons moved from
`~/.local/share/hicolor/...` (the previous broken location) to
`~/.local/share/icons/hicolor/...`. The user's siyuan install
was migrated as part of this fix.

Existing installs needed their icons moved from
`~/.local/share/hicolor/...` (the previous broken location) to
`~/.local/share/icons/...`. The user's siyuan install
was migrated as part of this fix.

## Bug fix: icon picker grabbed a tutorial asset

Slice 1 picked the largest PNG in the extracted tree. For
`siyuan-3.8.7` that grabbed a 358 KB tutorial screenshot from
`resources/guide/.../custom-icon1-...png` instead of the real
5 KB M logo at `usr/share/icons/hicolor/512x512/apps/siyuan.png`.

Real AppImages ship icons in conventional locations:

1. `.DirIcon` — the freedesktop canonical name for a directory
   icon, almost always a symlink into the hicolor tree.
2. The hicolor tree itself
   (`usr/share/icons/hicolor/<size>x<size>/apps/<id>.png`).
3. Top-level `<id>.png`, `icon.png`, `logo.png`,
   `icon-large.png`.

Slice 11 rewrites `icon::pick_best_icon` to walk this
precedence chain. The final fallback still picks the largest
PNG, but skips any path containing `/guide/`, `/tutorial/`,
`/help/`, `/docs/`, `/doc/`, or `/screenshots/` — the
directories that typically hold tutorial assets dwarfing the
real icon.

Tests: 7 new in `icon::tests` (picks_diricon_first,
picks_hicolor_when_no_diricon,
picks_largest_hicolor_size_available,
picks_top_level_when_no_diricon_or_hicolor,
fallback_skips_tutorial_directory, returns_none_when_no_pngs,
is_in_guide_dir_recognises_markers). 93 unit tests, all green.

End-to-end verified: `siyuan@3.8.7` reinstalled with the new
logic; the extracted icon at
`~/.local/share/icons/hicolor/512x512/apps/aim-siyuan.png`
now has md5 `5f3edced842be3554f95ce27b4714c4a` — same as the
hicolor tree's `siyuan.png`. File size 5625 bytes, 512×512 RGBA.

## Bug fix: `aim remove` left icons orphaned

Two bugs in `aim remove` conspired to leave icons behind.

**Bug A — design**: without `--purge`, the icon file was kept
even though the `.desktop` file (its only consumer) was deleted.
User saw `removed nvim (cache preserved)` and reasonably
expected "cache" to mean just the AppImage binary; the icon
lingered with no `.desktop` reference. Fixed by removing the
icon unconditionally — it's part of the install record, not
part of the cache. With `--purge`, both go (plus the AppImage
binary); without, only the AppImage binary stays.

**Bug B — filename mismatch in `remove_icons`**: the cleanup
function built the filename from the full `key` (`nvim@0.10.0`),
producing `aim-nvim@0.10.0.png`. But the slice-1 fix changed
`paths::icon_path` to strip `@version` (so the `.desktop`'s
`Icon=aim-nvim` matches), so the actual file is `aim-nvim.png`.
`remove_icons` was looking for a name that didn't exist on
disk — that's why even `--purge` was leaving icons behind.
Now `remove_icons` uses `id_for_key(key)` to strip the
version, matching what `icon_path` writes.

Tests: 2 new in `remove::tests`:
- `remove_default_matches_id_based_icon_filename` — locks in
  the alignment between `paths::id_for_key` and the icon file
  on disk.
- The existing `remove_default_preserves_cache` now also plants
  an icon and asserts it's gone after default remove.
94 unit tests, all green.

End-to-end verified: installed nvim with a fake AppImage, ran
`aim remove nvim` (no --purge). Icon was correctly removed
(`~/.local/share/icons/hicolor/512x512/apps/aim-nvim.png`
gone). The cached AppImage binary at
`~/.local/share/aim/apps/nvim/AppImage` survived, as designed.
Existing stale `aim-nvim.png` and `aim-marktext.png` orphans
(from before the fix) were removed by hand.

## Feature: `aim reinstall <id>`

After a default `aim remove` (no `--purge`), the cached
AppImage binary survives at `apps/<id@version>/AppImage` but
the state record is gone. Without a reinstall command, the
user would either re-download via `aim install --force
gh:owner/repo` or hand-feed the cached path to `aim install`,
neither of which is friendly.

Slice 12 adds `aim reinstall <target>` that wires the surviving
file back through the install pipeline. Two bugs caught while
implementing:

- The cached file is named `AppImage` (per install's atomic
  cache layout). Identity parsing falls back to filename stem
  when the AppImage has no embedded `AppImage.yml`, so passing
  the cache path verbatim identifies as `id="AppImage"`.
  Workaround: stage a copy with a meaningful basename before
  the install. When the original key carries a version, use
  `<id>-<version>.AppImage` (matches typical GitHub release
  asset names) so identity can recover the version via
  filename parsing too.

- `resolve_cached_key` had to handle both `apps/<id>/` (when
  the AppImage had no identifiable version) and
  `apps/<id>@<version>/` directories. With one match, use it;
  with multiple, error and ask the user to disambiguate.

Tests: 6 new in `reinstall::tests`
(resolve_cached_key_with_at_returns_verbatim,
resolve_cached_key_with_no_match_errors,
resolve_cached_key_with_multiple_matches_errors,
resolve_cached_key_with_versioned_match_prefers_at_form,
reinstall_after_default_remove_recovers_state,
reinstall_with_no_cache_errors_clearly,
reinstall_after_purge_fails). 102 unit + 4 integration green
total.

End-to-end verified on the user's siyuan@3.8.7: removed,
reinstalled from cache, key + version preserved
(`siyuan@3.8.7`), icon md5 matches the hicolor tree's
`siyuan.png` (`5f3edced…`), `.desktop` file references the
correct cached AppImage path.

Known limitation: per-app overrides (`env`, `extra_exec_args`,
`comment`, `categories`) are NOT preserved across
remove/reinstall — they live in the state record which is
deleted by `aim remove`. A future polish slice could keep a
backup `state.toml.bak` or a `removed/` section in state
that `reinstall` consults.

## Bug: stale `~/.cargo/bin/aim` re-introduced the wrong path

After the icons-dir fix landed, the user reported that a new
install (`DesktopEditors`) put its icon at
`~/.local/share/hicolor/512x512/apps/aim-DesktopEditors.png`
— missing the `icons/` segment again.

Root cause: the user had `~/.cargo/bin/aim` installed via
`cargo install --path` from a binary built on Oct 19 21:10 —
*between* the second icons fix
(`icons_dir = dirs::data_dir()`, no `icons/` segment) and the
third fix (added the `icons/` segment). The binary didn't
carry the third fix.

Fix: rebuilt release and reinstalled into `~/.cargo/bin/`
(`cargo install --path . --bin aim --force`). Verified with a
clean HOME: the new binary places icons at
`.../.local/share/icons/hicolor/...` ✓.

Lesson: when fixing the icons path, any out-of-tree install
of `aim` needs to be rebuilt for the fix to land. There is
no in-band way to detect this; a future polish slice could
add a `--check-path` self-check that compares `aim`'s
expected icon dir against what the OS theme lookup actually
consults.

The user's stale `aim-DesktopEditors.png` icon was migrated
by hand (move + state re-render + theme cache rebuild).

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

## Feature: `aim mime add/remove/list/default`

The user's question (xlsx → DesktopEditors as default) needed
three real mechanisms:

1. The `.desktop` file's `MimeType=` line (read by
   `update-desktop-database` to build `mimeinfo.cache`).
2. A way to set the OS default per MIME type (writes
   `~/.config/mimeapps.list`).
3. Both wired through `aim` so the user doesn't have to
   hand-edit `.desktop` files.

Added `mime_types: Vec<String>` to `AppRecord` (with
`#[serde(default)]` for state migration), taught
`desktop::render` to emit `MimeType=` when non-empty, and
added a new `src/mime.rs` with `add`, `remove`, `list`,
`set_default`. `set_default` deliberately writes
`mimeapps.list` directly rather than shelling out to
`xdg-mime default` — `xdg-utils` is not always installed, and
the file format is portable across all XDG-compliant DEs.

Two bugs caught while implementing:

- `set_default` did not require the install to have already
  declared the MIME type. Without `add` first, the OS rejects
  the association as undeclared. Now `set_default` errors with
  `aim::error::invalid_target` ("does not advertise '<mime>'")
  if the type isn't in the record.
- `rebuild_title_cache` was passing
  `dirs::data_dir().join(".local/share/applications")`,
  which under default data dir = `~/.local/share` produces
  `~/.local/share/.local/share/applications` (doubled). Fixed
  by threading the `Layout` and using
  `layout.apps_root_dir.join("applications")`.

Tests: 9 new in `mime::tests`. 111 unit + 4 integration
green total.

End-to-end verified: `DesktopEditors` is now wired up as the
default for `application/vnd.openxmlformats-officedocument.spreadsheetml.sheet`,
`application/vnd.ms-excel`, and
`application/vnd.oasis.opendocument.spreadsheet`. Both
`mimeapps.list` and `mimeinfo.cache` reflect the
association. `xdg-mime query default` returns
`aim-DesktopEditors.desktop`.

Known limitation: `aim` doesn't auto-suggest MIME types per
app — the user has to know that OnlyOffice handles xlsx (and
not, say, docx with the same `vd.openxmlformats-officedocument`
prefix). A future polish slice could extract MIME types from
the AppImage's embedded `.desktop` template (some ship one),
maintain a curated lookup table for popular apps, or accept
a `--mimetype` flag on `aim install`.

## Bug fix: `Exec=` line dropped file paths

Once MIME associations worked, double-clicking an `.xlsx` did
open DesktopEditors — but the file wasn't loaded. The `.desktop`
file's `Exec=` line was just the AppImage path with no file
placeholder:

```
Exec=env AIM_INTEGRATED=1 sh -c "/home/mi/.local/share/aim/apps/DesktopEditors/AppImage"
```

Per the freedesktop Desktop Entry Spec, `Exec=` must include
at least one of `%f`, `%F`, `%u`, `%U` for the desktop
environment to forward file paths. Without it, the AppImage
gets the file-MIME-assoc default and launches, but the path
is dropped on the floor.

Fix: `desktop::render` now always appends `%F` (multiple file
paths, most robust default) at the end of the shell command
unless the user's `extra_exec_args` already contains a
placeholder — in which case their intent is preserved exactly.

Tests: 2 new (`user_supplied_placeholder_is_preserved`,
`file_placeholder_always_present`) plus the existing two
Exec-line assertions updated. 113 unit + 4 integration green.

End-to-end: `aim integrate DesktopEditors` re-renders the
desktop file; `desktop-file-validate` passes; double-clicking
an xlsx now forwards the file path.

### KDE cache caveat

After the `.desktop` fix, on KDE the file-manager still served
the stale `.desktop` until `kbuildsycoca6 --noincremental` was
run. KDE's `ksycoca` (System Configuration Cache) is the DE's
own index of `.desktop` files and is independent of
`mimeinfo.cache`. Until it's rebuilt, KDE continues to use the
old (placeholder-less) Exec line.

A future polish slice could have `aim integrate` /
`aim mime add` shell out to `kbuildsycoca6` (KDE 6) or
`kbuildsycoca5` (KDE 5) automatically. For now the user has
to know the trick.

Recipe after editing `.desktop`:

```
kbuildsycoca6 --noincremental   # KDE 6
# or kbuildsycoca5 --noincremental   # KDE 5
update-desktop-database ~/.local/share/applications
appstreamcli refresh-cache      # optional
```

If still stale: restart the file manager (`dolphin` for KDE,
`nautilus` for GNOME, `thunar` for XFCE) or log out / in.

### Bug: `sh -c "AppImage"` swallowed file arguments

After the `%F` placeholder was added, double-clicking an xlsx
*launched* DesktopEditors but *never opened the file* — the
start screen appeared every time.

Root cause: the Exec line was wrapped in `sh -c`:

```
Exec=env AIM_INTEGRATED=1 sh -c "/path/to/AppImage" %F
```

After the DE substituted `%F` and ran the line:

```
env AIM_INTEGRATED=1 sh -c "/path/to/AppImage" /home/mi/foo.xlsx
```

But `sh -c CMD ARG` makes ARG the shell's `$0`, not an extra
argument to CMD. The shell parsed the command string as
`/path/to/AppImage` and exec'd it with an empty argv. The
file path disappeared. The AppImage launched with no args,
showed the start screen, and silently waited.

Proof (run in any shell):

```
$ sh -c 'echo "argv count: $#  argv[0]: $0  argv[1]: $1"' /file.xlsx
argv count: 0  argv[0]: /file.xlsx  argv[1]:
```

`argv count: 0` — the file is not a positional argument, it's
the shell's own `$0`.

Fix: drop the `sh -c` wrapper. The Exec line is just
`env … /AppImage %F`; `env VAR=val command args…` exec's
`command` with `args…` as its argv. The AppImage then receives
the file path as `argv[1]`, and the type-2 runtime
substitutes `%F` into the embedded `.desktop`'s `Exec=`
field, and the actual app's wrapper script propagates the
file down to its main binary.

End-to-end:

```
$ tr '\0' ' ' < /proc/<DesktopEditors-pid>/cmdline
./DesktopEditors /home/mi/Downloads/...xlsx
```

The file is in argv. The DesktopEditors process spins at
~95% CPU and `~/.local/share/onlyoffice/desktopeditors/data/
recents.xml` gets touched — OnlyOffice tracks the open file
in its recents list, which only happens for files actually
opened (not for the start screen).

Tests: existing Exec-line assertions updated to the new
format; no new tests added (the format check already locks
the new format in). 115 unit + 4 integration green.
