//! `aim upgrade` — replace an installed AppImage with a newer
//! release. Currently supports `gh:` sources only.

use std::cmp::Ordering;
use std::os::unix::fs::PermissionsExt;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::check;
use crate::desktop;
use crate::download;
use crate::error::{Error, Result};
use crate::gh;
use crate::hash;
use crate::icon;
use crate::identity;
use crate::paths::{Layout, ensure_dir};
use crate::staging;
use crate::state::{AppRecord, Source, State};

/// Run `aim upgrade <id>`.
pub fn run(target: &str, layout: &Layout) -> Result<()> {
    let mut state = State::load(&layout.state_file)?;
    let (old_key, old_record) = state.lookup(target)?;

    // 1. Refuse non-`gh:` sources for slice 10; the design-time
    //    discussion was that upgrades ride the same resolver as
    //    install. Url sources could be handled by an
    //    ETag/If-Modified-Since dance in a future slice.
    let (owner, repo) = match &old_record.source {
        Source::Gh { owner, repo } => (owner.clone(), repo.clone()),
        other => {
            return Err(Error::InvalidTarget {
                message: format!(
                    "upgrades are only supported for gh: installs (this one is {other:?})"
                ),
            });
        }
    };

    // 2. Resolve the latest release.
    let gh_ref = gh::GhRef {
        owner: owner.clone(),
        repo: repo.clone(),
        version: None,
    };
    let (upstream_tag, asset) = gh::resolve_release(&gh_ref)?;

    // 3. Compare versions. Bail early for up-to-date / ahead.
    let local_v = check::parse_version(old_record.version.as_deref().unwrap_or(""));
    let upstream_v = check::parse_version(&upstream_tag);
    match (local_v, upstream_v) {
        (Some(l), Some(u)) => match check::compare(&l, &u) {
            Ordering::Equal => {
                println!("already up-to-date ({upstream_tag})");
                return Ok(());
            }
            Ordering::Greater => {
                return Err(Error::InvalidTarget {
                    message: format!(
                        "local {} is ahead of upstream {}; refusing to downgrade",
                        old_record.version.as_deref().unwrap_or("?"),
                        upstream_tag
                    ),
                });
            }
            Ordering::Less => {} // proceed
        },
        _ => {
            return Err(Error::InvalidTarget {
                message: format!(
                    "cannot compare versions: local={:?} upstream={upstream_tag}",
                    old_record.version
                ),
            });
        }
    }
    eprintln!(
        "upgrading {old_key} → {} ({upstream_tag})",
        asset.name
    );

    // 4. Download the new asset.
    let cache_dir = layout.data_dir.join("downloads");
    let dl = download::download(&asset.url, &cache_dir, Some(&asset.name))?;

    // 5. Identify the new AppImage (may have a different id or
    //    version than the asset filename suggests).
    let ident = identity::identify(&dl.path)?;
    let new_id = ident.id.clone();
    let new_version = ident.version.clone();
    let new_key = match &new_version {
        Some(v) => format!("{new_id}@{v}"),
        None => new_id.clone(),
    };

    // 6. Bail if the new key already exists in state.
    if state.apps.contains_key(&new_key) {
        return Err(Error::AlreadyInstalled {
            id: new_id,
            version: new_version.unwrap_or_else(|| "?".to_string()),
        });
    }

    // 7. Hash + size the new file.
    let (new_hash, new_size) = hash::hash_file(&dl.path)?;

    // 8. Build the new record, preserving per-app overrides and
    //    the integration flag.
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    let mut new_record = AppRecord {
        id: new_id.clone(),
        version: new_version.clone(),
        sha256: new_hash.clone(),
        size_bytes: new_size,
        integrated: old_record.integrated,
        source: Source::Gh { owner, repo },
        icon_size_px: None,
        icon_path: None,
        env: old_record.env.clone(),
        extra_exec_args: old_record.extra_exec_args.clone(),
        comment: old_record.comment.clone(),
        categories: old_record.categories.clone(),
        mime_types: old_record.mime_types.clone(),
        installed_at_secs: now,
    };

    // 9. Extract icon (best-effort; mirrors the install pipeline).
    if let Some(extracted) = icon::extract(&new_key, &dl.path, layout)? {
        new_record.icon_size_px = Some(extracted.size_px);
        if let Ok(rel) = extracted.on_disk.strip_prefix(&layout.icons_dir) {
            new_record.icon_path = Some(rel.to_string_lossy().into_owned());
        }
    }

    // 10. Atomic cache placement: write to a tempfile inside the
    //     new app dir, chmod 0755, persist (rename) to AppImage.
    let new_app_dir = layout.app_dir(&new_key);
    ensure_dir(&new_app_dir)?;
    let dest = new_app_dir.join("AppImage");
    let tmp = tempfile::Builder::new()
        .prefix("AppImage.")
        .tempfile_in(&new_app_dir)?;
    std::fs::copy(&dl.path, tmp.path())?;
    let mut perm = std::fs::metadata(tmp.path())?.permissions();
    perm.set_mode(0o755);
    std::fs::set_permissions(tmp.path(), perm)?;
    tmp.persist(&dest).map_err(|e| Error::Path {
        path: dest.clone(),
        source: e.error,
    })?;

    // 11. Re-render the integration artefacts under the new key
    //     when integrated.
    if new_record.integrated {
        let desktop_text = desktop::render(&new_record, &dest);
        staging::atomic_write(
            &layout.desktop_file(&new_key),
            desktop_text.as_bytes(),
        )?;
        staging::symlink(&dest, &layout.bin_link(&new_key))?;
    }

    // 12. Remove the OLD integration artefacts (if any), then drop
    //     the OLD app dir. Done after step 11 so a partial
    //     failure leaves the system in a state with the new
    //     artefacts + the old key still pointing at the new
    //     cache; the user can `aim remove <new>` and re-install.
    if old_record.integrated {
        staging::remove_any(&layout.desktop_file(&old_key));
        staging::remove_any(&layout.bin_link(&old_key));
    }
    if old_key != new_key {
        let old_app_dir = layout.app_dir(&old_key);
        if old_app_dir.exists() {
            let _ = std::fs::remove_dir_all(&old_app_dir);
        }
    }

    // 13. Update state: drop the old record, insert the new one.
    state.apps.remove(&old_key);
    state.apps.insert(new_key.clone(), new_record);
    state.save(&layout.state_file)?;

    println!("upgraded {old_key} → {new_key} (sha256={})", &new_hash[..12]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    // No network-using unit tests here — `run` calls
    // `gh::resolve_release` and `download::download`, both of
    // which hit the network. The end-to-end flow is verified
    // manually in the slice-10 manual run.
}
