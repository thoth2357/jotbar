//! Local note store: one plain Markdown file per note plus a small index.
//!
//! Layout of the data directory:
//!   notes/<id>.md   note content, editable by any other tool
//!   index.json      tab order, timestamps, hashes, tombstones and sync bases

use crate::{atomic_write, content_hash, now_ms, valid_id};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NoteMeta {
    pub id: String,
    pub title: String,
    pub order: i64,
    pub updated_at: i64,
    pub deleted: bool,
    pub hash: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct SearchHit {
    pub id: String,
    pub title: String,
    /// Character offset of the match inside the note.
    pub offset: usize,
    pub line: usize,
    pub snippet: String,
}

#[derive(Default, Serialize, Deserialize)]
struct Index {
    notes: Vec<NoteMeta>,
    /// id -> hash both sides agreed on at the last successful sync.
    #[serde(default)]
    synced: HashMap<String, String>,
}

pub struct Store {
    dir: PathBuf,
    index: Index,
}

pub fn title_of(content: &str) -> String {
    let line = content
        .lines()
        .map(|l| l.trim().trim_start_matches('#').trim())
        .find(|l| !l.is_empty())
        .unwrap_or("Untitled");
    let mut t: String = line.chars().take(40).collect();
    if line.chars().count() > 40 {
        t.push('…');
    }
    t
}

impl Store {
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self> {
        let dir = dir.into();
        fs::create_dir_all(dir.join("notes"))
            .with_context(|| format!("creating {}", dir.display()))?;
        let index_path = dir.join("index.json");
        let index = if index_path.exists() {
            serde_json::from_slice(&fs::read(&index_path)?).context("reading index.json")?
        } else {
            Index::default()
        };
        let mut store = Store { dir, index };
        store.reconcile()?;
        if store.list().is_empty() {
            store.create("")?;
        }
        Ok(store)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn note_path(&self, id: &str) -> PathBuf {
        self.dir.join("notes").join(format!("{id}.md"))
    }

    /// Pick up edits made outside the app (another editor, Syncthing, git):
    /// new .md files become notes, changed files get a new hash, and missing
    /// files become deletions.
    fn reconcile(&mut self) -> Result<()> {
        let mut changed = false;
        let mut seen = std::collections::HashSet::new();
        for entry in fs::read_dir(self.dir.join("notes"))? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|s| s.to_str()).map(String::from) else {
                continue;
            };
            if !valid_id(&id) {
                continue;
            }
            let content = fs::read_to_string(&path).unwrap_or_default();
            let hash = content_hash(&content, false);
            seen.insert(id.clone());
            match self.index.notes.iter_mut().find(|n| n.id == id) {
                Some(n) if n.hash == hash && !n.deleted => {}
                Some(n) => {
                    n.hash = hash;
                    n.deleted = false;
                    n.title = title_of(&content);
                    n.updated_at = now_ms();
                    changed = true;
                }
                None => {
                    let order = self.next_order();
                    self.index.notes.push(NoteMeta {
                        id,
                        title: title_of(&content),
                        order,
                        updated_at: now_ms(),
                        deleted: false,
                        hash,
                    });
                    changed = true;
                }
            }
        }
        for n in self.index.notes.iter_mut() {
            if !n.deleted && !seen.contains(&n.id) {
                n.deleted = true;
                n.hash = content_hash("", true);
                n.updated_at = now_ms();
                changed = true;
            }
        }
        if changed {
            self.save_index()?;
        }
        Ok(())
    }

    fn save_index(&self) -> Result<()> {
        atomic_write(
            &self.dir.join("index.json"),
            &serde_json::to_vec_pretty(&self.index)?,
        )?;
        Ok(())
    }

    fn next_order(&self) -> i64 {
        self.index.notes.iter().map(|n| n.order).max().unwrap_or(0) + 1
    }

    fn meta_mut(&mut self, id: &str) -> Result<&mut NoteMeta> {
        self.index
            .notes
            .iter_mut()
            .find(|n| n.id == id)
            .with_context(|| format!("no note {id}"))
    }

    /// Live notes in tab order.
    pub fn list(&self) -> Vec<NoteMeta> {
        let mut v: Vec<_> = self.index.notes.iter().filter(|n| !n.deleted).cloned().collect();
        v.sort_by_key(|n| n.order);
        v
    }

    /// Every note, including tombstones (used by sync).
    pub fn all_meta(&self) -> Vec<NoteMeta> {
        self.index.notes.clone()
    }

    pub fn get_meta(&self, id: &str) -> Option<&NoteMeta> {
        self.index.notes.iter().find(|n| n.id == id)
    }

    pub fn read(&self, id: &str) -> Result<String> {
        if !valid_id(id) {
            bail!("invalid id");
        }
        match self.get_meta(id) {
            Some(m) if m.deleted => Ok(String::new()),
            Some(_) => Ok(fs::read_to_string(self.note_path(id)).unwrap_or_default()),
            None => bail!("no note {id}"),
        }
    }

    pub fn create(&mut self, content: &str) -> Result<NoteMeta> {
        self.create_after(content, None)
    }

    /// Create a note, placed right after `after` in tab order if given.
    pub fn create_after(&mut self, content: &str, after: Option<&str>) -> Result<NoteMeta> {
        let id = uuid::Uuid::new_v4().to_string();
        let order = match after.and_then(|a| self.get_meta(a)).map(|m| m.order) {
            Some(o) => {
                for n in self.index.notes.iter_mut().filter(|n| n.order > o) {
                    n.order += 1;
                }
                o + 1
            }
            None => self.next_order(),
        };
        atomic_write(&self.note_path(&id), content.as_bytes())?;
        let meta = NoteMeta {
            id,
            title: title_of(content),
            order,
            updated_at: now_ms(),
            deleted: false,
            hash: content_hash(content, false),
        };
        self.index.notes.push(meta.clone());
        self.save_index()?;
        Ok(meta)
    }

    pub fn write(&mut self, id: &str, content: &str) -> Result<NoteMeta> {
        let hash = content_hash(content, false);
        let path = self.note_path(id);
        let m = self.meta_mut(id)?;
        if m.hash == hash && !m.deleted {
            return Ok(m.clone());
        }
        m.hash = hash;
        m.deleted = false;
        m.title = title_of(content);
        m.updated_at = now_ms();
        let out = m.clone();
        atomic_write(&path, content.as_bytes())?;
        self.save_index()?;
        Ok(out)
    }

    pub fn delete(&mut self, id: &str) -> Result<()> {
        let path = self.note_path(id);
        let m = self.meta_mut(id)?;
        m.deleted = true;
        m.hash = content_hash("", true);
        m.updated_at = now_ms();
        let _ = fs::remove_file(path);
        self.save_index()?;
        if self.list().is_empty() {
            self.create("")?;
        }
        Ok(())
    }

    pub fn reorder(&mut self, ids: &[String]) -> Result<()> {
        for (i, id) in ids.iter().enumerate() {
            if let Ok(m) = self.meta_mut(id) {
                m.order = i as i64;
            }
        }
        self.save_index()
    }

    /// Case-insensitive search across every note's title and body.
    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchHit> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return vec![];
        }
        let mut hits = vec![];
        for meta in self.list() {
            let Ok(content) = self.read(&meta.id) else { continue };
            let mut offset = 0usize;
            for (lineno, line) in content.split('\n').enumerate() {
                let lower = line.to_lowercase();
                // Only search lines whose lowercase form keeps the same
                // char count, so offsets map back correctly.
                if let Some(byte) = lower.find(&q) {
                    if lower.chars().count() == line.chars().count() {
                        let col = lower[..byte].chars().count();
                        let start = col.saturating_sub(30);
                        let snippet: String = line.chars().skip(start).take(90).collect();
                        hits.push(SearchHit {
                            id: meta.id.clone(),
                            title: meta.title.clone(),
                            offset: offset + col,
                            line: lineno,
                            snippet: snippet.trim().to_string(),
                        });
                        if hits.len() >= limit {
                            return hits;
                        }
                    }
                }
                offset += line.chars().count() + 1;
            }
        }
        hits
    }

    // ---- sync support ----

    pub fn synced_hash(&self, id: &str) -> Option<String> {
        self.index.synced.get(id).cloned()
    }

    pub fn synced_map(&self) -> HashMap<String, String> {
        self.index.synced.clone()
    }

    pub fn set_synced(&mut self, id: &str, hash: &str) -> Result<()> {
        if self.index.synced.get(id).map(String::as_str) != Some(hash) {
            self.index.synced.insert(id.to_string(), hash.to_string());
            self.save_index()?;
        }
        Ok(())
    }

    /// Overwrite a note with the server's version (creating it if new).
    pub fn apply_remote(&mut self, id: &str, content: &str, deleted: bool, updated_at: i64) -> Result<()> {
        if !valid_id(id) {
            bail!("server sent invalid id {id:?}");
        }
        let path = self.note_path(id);
        if deleted {
            let _ = fs::remove_file(&path);
        } else {
            atomic_write(&path, content.as_bytes())?;
        }
        let hash = content_hash(content, deleted);
        let order = self.next_order();
        match self.index.notes.iter_mut().find(|n| n.id == id) {
            Some(m) => {
                m.hash = hash;
                m.deleted = deleted;
                m.title = title_of(content);
                m.updated_at = updated_at;
            }
            None => self.index.notes.push(NoteMeta {
                id: id.to_string(),
                title: title_of(content),
                order,
                updated_at,
                deleted,
                hash,
            }),
        }
        self.save_index()?;
        if self.list().is_empty() {
            self.create("")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_write_delete_search() {
        let tmp = tempfile::tempdir().unwrap();
        let mut s = Store::open(tmp.path()).unwrap();
        assert_eq!(s.list().len(), 1); // starts with one blank note
        let a = s.create("# Groceries\nmilk\neggs").unwrap();
        assert_eq!(a.title, "Groceries");
        let a2 = s.write(&a.id, "# Groceries\nmilk\nEggs and bread").unwrap();
        assert_ne!(a.hash, a2.hash);
        let hits = s.search("bread", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].line, 2);
        let content = s.read(&a.id).unwrap();
        let off = hits[0].offset;
        assert_eq!(&content.chars().skip(off).take(5).collect::<String>(), "bread");
        s.delete(&a.id).unwrap();
        assert!(s.list().iter().all(|n| n.id != a.id));
        // reopening keeps the tombstone
        let s2 = Store::open(tmp.path()).unwrap();
        assert!(s2.get_meta(&a.id).unwrap().deleted);
    }

    #[test]
    fn picks_up_external_files() {
        let tmp = tempfile::tempdir().unwrap();
        drop(Store::open(tmp.path()).unwrap());
        fs::write(tmp.path().join("notes/from-elsewhere.md"), "hello").unwrap();
        let s = Store::open(tmp.path()).unwrap();
        assert!(s.list().iter().any(|n| n.id == "from-elsewhere"));
    }
}
