//! Folder target: notes live as `<id>.md` files in any directory. Point it at
//! a Syncthing / Nextcloud / Dropbox folder and that tool does the transport.
//! The sync server uses this same storage.

use super::{decode_note, encode_note};
use crate::{atomic_write, valid_id, PutNote, RemoteMeta, RemoteNote};
use anyhow::{bail, Context, Result};
use std::path::PathBuf;

pub struct FolderBackend {
    dir: PathBuf,
}

pub enum Put {
    Written(RemoteMeta),
    Conflict(RemoteNote),
}

impl FolderBackend {
    pub fn new(dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        Ok(FolderBackend { dir })
    }

    fn path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.md"))
    }

    pub fn list(&self) -> Result<Vec<RemoteMeta>> {
        let mut out = vec![];
        for e in std::fs::read_dir(&self.dir).with_context(|| format!("reading {}", self.dir.display()))? {
            let name = e?.file_name();
            let Some(id) = name.to_str().and_then(|n| n.strip_suffix(".md")) else { continue };
            if !valid_id(id) {
                continue;
            }
            if let Some(n) = self.get(id)? {
                out.push(RemoteMeta { id: n.id, updated_at: n.updated_at, hash: n.hash, deleted: n.deleted });
            }
        }
        Ok(out)
    }

    pub fn get(&self, id: &str) -> Result<Option<RemoteNote>> {
        if !valid_id(id) {
            bail!("invalid id");
        }
        match std::fs::read_to_string(self.path(id)) {
            Ok(raw) => Ok(Some(decode_note(id, &raw))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Compare-and-swap write. Callers must serialise writes to one folder
    /// (the server holds a lock; the app runs one sync at a time).
    pub fn put(&self, id: &str, body: &PutNote) -> Result<Put> {
        let hash = crate::content_hash(&body.content, body.deleted);
        if let Some(cur) = self.get(id)? {
            if cur.hash == hash {
                return Ok(Put::Written(RemoteMeta { id: id.into(), updated_at: cur.updated_at, hash, deleted: cur.deleted }));
            }
            if body.base_hash.as_deref() != Some(cur.hash.as_str()) {
                return Ok(Put::Conflict(cur));
            }
        }
        atomic_write(&self.path(id), encode_note(&body.content, body.updated_at, body.deleted).as_bytes())?;
        Ok(Put::Written(RemoteMeta { id: id.into(), updated_at: body.updated_at, hash, deleted: body.deleted }))
    }
}
