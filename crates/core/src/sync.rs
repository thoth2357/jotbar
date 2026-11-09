//! Client side of the sync protocol.
//!
//! Three-way comparison per note: local hash, server hash, and the base hash
//! both agreed on at the last sync. Whichever side moved away from the base
//! wins. If both moved, the server copy is pulled and the local text is kept
//! as a separate "conflicted copy" note, so no edit is ever lost.

use crate::remote::{PutResult, Remote};
use crate::{content_hash, PutNote, RemoteMeta, Store};
use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::{BTreeSet, HashMap};
use std::sync::Mutex;

#[derive(Debug, Default, Clone, Serialize)]
pub struct SyncReport {
    pub pushed: usize,
    pub pulled: usize,
    pub conflicts: usize,
    /// True if any local note changed, so the UI should refresh.
    pub changed_local: bool,
}

fn lock(store: &Mutex<Store>) -> std::sync::MutexGuard<'_, Store> {
    store.lock().unwrap_or_else(|e| e.into_inner())
}

/// Run one full sync pass. The store lock is only held for short local
/// operations, never across network calls, so the editor stays responsive.
pub async fn sync(store: &Mutex<Store>, client: &mut Remote) -> Result<SyncReport> {
    let remote: HashMap<String, RemoteMeta> = client
        .list()
        .await
        .context("listing notes on the sync target")?
        .into_iter()
        .filter(|m| crate::valid_id(&m.id))
        .map(|m| (m.id.clone(), m))
        .collect();
    let (local, bases) = {
        let s = lock(store);
        let local: HashMap<_, _> = s.all_meta().into_iter().map(|m| (m.id.clone(), m)).collect();
        (local, s.synced_map())
    };
    let empty = content_hash("", false);
    let ids: BTreeSet<&String> = remote.keys().chain(local.keys()).collect();
    let mut rep = SyncReport::default();

    for id in ids {
        let l = local.get(id);
        let r = remote.get(id);
        let base = bases.get(id);
        let lh = l.map(|m| &m.hash);
        let rh = r.map(|m| &m.hash);

        if lh == rh {
            if let Some(h) = lh {
                lock(store).set_synced(id, h)?;
            }
            continue;
        }
        // A blank note nobody has typed in yet stays local.
        if r.is_none() && base.is_none() && lh == Some(&empty) {
            continue;
        }

        let local_changed = lh != base;
        let remote_changed = rh != base;
        let action = match (local_changed, remote_changed) {
            (true, false) => Action::Push,
            (false, true) => Action::Pull,
            _ => match (l, r) {
                (_, None) => Action::Push,
                (None, _) => Action::Pull,
                (Some(l), Some(r)) if l.deleted && !r.deleted => Action::Pull,
                (Some(l), Some(r)) if r.deleted && !l.deleted => Action::Push,
                _ => Action::Conflict,
            },
        };

        match action {
            Action::Push => {
                let l = l.expect("push implies a local note");
                let content = if l.deleted { String::new() } else { lock(store).read(id)? };
                // The note may have been edited since we took the snapshot;
                // push what is on disk now and record that hash.
                let hash = content_hash(&content, l.deleted);
                let body = PutNote {
                    content,
                    updated_at: l.updated_at,
                    deleted: l.deleted,
                    base_hash: rh.cloned(),
                };
                if let PutResult::Ok = client.put(id, &body).await? {
                    lock(store).set_synced(id, &hash)?;
                    rep.pushed += 1;
                }
                // On 409 the server changed underneath us; the next pass sees
                // both sides changed and resolves it as a conflict.
            }
            Action::Pull => {
                let note = client.get(id).await?;
                let mut s = lock(store);
                // Skip if the user edited this note while we were fetching.
                if s.get_meta(id).map(|m| &m.hash) != lh {
                    continue;
                }
                s.apply_remote(id, &note.content, note.deleted, note.updated_at)?;
                s.set_synced(id, &note.hash)?;
                rep.pulled += 1;
                rep.changed_local = true;
            }
            Action::Conflict => {
                let note = client.get(id).await?;
                let mut s = lock(store);
                if s.get_meta(id).map(|m| &m.hash) != lh {
                    continue;
                }
                let mine = s.read(id)?;
                s.apply_remote(id, &note.content, note.deleted, note.updated_at)?;
                s.set_synced(id, &note.hash)?;
                s.create_after(&format!("⚠ Conflicted copy\n\n{mine}"), Some(id))?;
                rep.conflicts += 1;
                rep.changed_local = true;
            }
        }
    }
    Ok(rep)
}

enum Action {
    Push,
    Pull,
    Conflict,
}
