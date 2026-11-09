//! Local note store: one plain Markdown file per note plus a small index.
//!
//! Layout of the data directory:
//!   notes/<id>.md   note content, editable by any other tool
//!   index.json      timestamps, hashes, tombstones and sync bases
//!
//! Tags are ordinary `#hashtags` inside the note text, so they travel with
//! the Markdown to every sync target and stay readable anywhere.

use crate::{atomic_write, content_hash, now_ms, valid_id};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NoteMeta {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub created_at: i64,
    pub updated_at: i64,
    pub deleted: bool,
    pub hash: String,
}

/// What the note list shows for each note.
#[derive(Clone, Debug, Serialize)]
pub struct NoteSummary {
    pub id: String,
    pub title: String,
    /// Text after the title line, shortened, with tag-only lines removed.
    pub preview: String,
    pub tags: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct NoteList {
    pub notes: Vec<NoteSummary>,
    /// Every tag across all notes with its count, most used first.
    pub tags: Vec<(String, usize)>,
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

fn is_tag_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '/')
}

/// `#tags` in the text, lowercased and de-duplicated in order of appearance.
/// Markdown headings (`# Title`) and things like `C#` or `url#anchor` are not tags.
pub fn tags_of(content: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let chars: Vec<char> = content.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let starts_word = i == 0 || chars[i - 1].is_whitespace() || chars[i - 1] == '(';
        if chars[i] == '#' && starts_word {
            let mut j = i + 1;
            while j < chars.len() && is_tag_char(chars[j]) {
                j += 1;
            }
            let tag: String = chars[i + 1..j].iter().collect::<String>();
            let tag = tag.trim_end_matches(['-', '/']).to_lowercase();
            // Must contain a letter, so "#1" or "#2024" in prose don't count.
            if tag.chars().any(char::is_alphabetic) && !out.contains(&tag) {
                out.push(tag);
            }
            i = j.max(i + 1);
        } else {
            i += 1;
        }
    }
    out
}

fn is_tag_line(line: &str) -> bool {
    let t = line.trim();
    !t.is_empty() && t.split_whitespace().all(|w| w.starts_with('#') && w.len() > 1 && !w.starts_with("##"))
        && !t.starts_with("# ")
}

/// A line for display: inline `#tags` removed (they are shown as chips).
fn strip_tags(line: &str) -> String {
    let tags = tags_of(line);
    line.split_whitespace()
        .filter(|w| {
            let t = w.trim_start_matches('(').trim_start_matches('#').trim_end_matches([',', '.', ';', ':', ')', '-', '/']);
            !(w.starts_with('#') || w.starts_with("(#")) || !tags.contains(&t.to_lowercase())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn title_of(content: &str) -> String {
    let line = content
        .lines()
        .filter(|l| !is_tag_line(l))
        .map(|l| strip_tags(l.trim().trim_start_matches('#').trim()))
        .find(|l| !l.is_empty())
        .unwrap_or_else(|| "Untitled".into());
    let mut t: String = line.chars().take(60).collect();
    if line.chars().count() > 60 {
        t.push('…');
    }
    t
}

fn preview_of(content: &str) -> String {
    let mut lines = content.lines().filter(|l| !l.trim().is_empty() && !is_tag_line(l));
    lines.next(); // the title
    let joined = lines.map(strip_tags).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" · ");
    let mut p: String = joined.chars().take(140).collect();
    if joined.chars().count() > 140 {
        p.push('…');
    }
    p
}

/// Append `#tags` that aren't already in the text as a final line.
pub fn with_tags(content: &str, tags: &[String]) -> String {
    let have = tags_of(content);
    let add: Vec<String> = tags
        .iter()
        .map(|t| t.trim().trim_start_matches('#').to_lowercase())
        .filter(|t| !t.is_empty() && t.chars().all(is_tag_char) && !have.contains(t))
        .fold(vec![], |mut v, t| {
            if !v.contains(&t) {
                v.push(t);
            }
            v
        });
    if add.is_empty() {
        return content.to_string();
    }
    let line = add.iter().map(|t| format!("#{t}")).collect::<Vec<_>>().join(" ");
    let body = content.trim_end();
    if body.is_empty() {
        line
    } else {
        format!("{body}\n\n{line}")
    }
}

impl Store {
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self> {
        let dir = dir.into();
        fs::create_dir_all(dir.join("notes"))
            .with_context(|| format!("creating {}", dir.display()))?;
        let index_path = dir.join("index.json");
        let mut index: Index = if index_path.exists() {
            serde_json::from_slice(&fs::read(&index_path)?).context("reading index.json")?
        } else {
            Index::default()
        };
        for n in index.notes.iter_mut().filter(|n| n.created_at == 0) {
            n.created_at = n.updated_at;
        }
        let mut store = Store { dir, index };
        store.reconcile()?;
        Ok(store)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn note_path(&self, id: &str) -> PathBuf {
        self.dir.join("notes").join(format!("{id}.md"))
    }

    /// Pick up edits made outside the app (another editor, git, a script):
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
                    let t = fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_millis() as i64)
                        .unwrap_or_else(now_ms);
                    self.index.notes.push(NoteMeta {
                        id,
                        title: title_of(&content),
                        created_at: t,
                        updated_at: t,
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

    fn meta_mut(&mut self, id: &str) -> Result<&mut NoteMeta> {
        self.index
            .notes
            .iter_mut()
            .find(|n| n.id == id)
            .with_context(|| format!("no note {id}"))
    }

    /// Live notes, newest first.
    pub fn list(&self) -> Vec<NoteMeta> {
        let mut v: Vec<_> = self.index.notes.iter().filter(|n| !n.deleted).cloned().collect();
        v.sort_by_key(|n| std::cmp::Reverse(n.created_at));
        v
    }

    /// The note list, optionally filtered by a text query (title and body,
    /// case-insensitive) and by tags (a note must have all of them).
    pub fn summaries(&self, query: &str, tags: &[String]) -> NoteList {
        let q = query.trim().to_lowercase();
        let want: Vec<String> = tags.iter().map(|t| t.trim_start_matches('#').to_lowercase()).collect();
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        let mut notes = vec![];
        for m in self.list() {
            let content = self.read(&m.id).unwrap_or_default();
            let note_tags = tags_of(&content);
            for t in &note_tags {
                *counts.entry(t.clone()).or_default() += 1;
            }
            if !want.iter().all(|w| note_tags.contains(w)) {
                continue;
            }
            if !q.is_empty() && !content.to_lowercase().contains(&q) {
                continue;
            }
            notes.push(NoteSummary {
                id: m.id.clone(),
                title: title_of(&content),
                preview: preview_of(&content),
                tags: note_tags,
                created_at: m.created_at,
                updated_at: m.updated_at,
            });
        }
        let mut tags: Vec<(String, usize)> = counts.into_iter().collect();
        tags.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        NoteList { notes, tags }
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
        let id = uuid::Uuid::new_v4().to_string();
        atomic_write(&self.note_path(&id), content.as_bytes())?;
        let now = now_ms();
        let meta = NoteMeta {
            id,
            title: title_of(content),
            created_at: now,
            updated_at: now,
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
        self.save_index()
    }

    // ---- sync support ----

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

    /// Overwrite a note with the target's version (creating it if new).
    pub fn apply_remote(&mut self, note: &crate::RemoteNote) -> Result<()> {
        let id = note.id.as_str();
        if !valid_id(id) {
            bail!("sync target sent invalid id {id:?}");
        }
        let path = self.note_path(id);
        if note.deleted {
            let _ = fs::remove_file(&path);
        } else {
            atomic_write(&path, note.content.as_bytes())?;
        }
        let hash = content_hash(&note.content, note.deleted);
        let created = if note.created_at > 0 { note.created_at } else { note.updated_at };
        match self.index.notes.iter_mut().find(|n| n.id == id) {
            Some(m) => {
                m.hash = hash;
                m.deleted = note.deleted;
                m.title = title_of(&note.content);
                m.updated_at = note.updated_at;
                if note.created_at > 0 {
                    m.created_at = note.created_at;
                }
            }
            None => self.index.notes.push(NoteMeta {
                id: id.to_string(),
                title: title_of(&note.content),
                created_at: created,
                updated_at: note.updated_at,
                deleted: note.deleted,
                hash,
            }),
        }
        self.save_index()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_write_delete_filter() {
        let tmp = tempfile::tempdir().unwrap();
        let mut s = Store::open(tmp.path()).unwrap();
        assert!(s.list().is_empty());
        let a = s.create(&with_tags("# Groceries\nmilk\neggs", &["shopping".into(), "#Home".into()])).unwrap();
        assert_eq!(a.title, "Groceries");
        assert_eq!(s.read(&a.id).unwrap(), "# Groceries\nmilk\neggs\n\n#shopping #home");
        std::thread::sleep(std::time::Duration::from_millis(3));
        let b = s.create("Call Ada about the #work demo").unwrap();

        let all = s.summaries("", &[]);
        assert_eq!(all.notes[0].id, b.id, "newest first");
        assert_eq!(all.notes[1].preview, "milk · eggs");
        assert_eq!(all.notes[1].tags, vec!["shopping", "home"]);
        assert_eq!(all.tags.len(), 3);

        assert_eq!(s.summaries("EGGS", &[]).notes.len(), 1);
        assert_eq!(s.summaries("", &["work".into()]).notes[0].id, b.id);
        assert!(s.summaries("", &["work".into(), "home".into()]).notes.is_empty());

        let a2 = s.write(&a.id, "# Groceries\nmilk\neggs and bread").unwrap();
        assert_ne!(a.hash, a2.hash);
        assert_eq!(a2.created_at, a.created_at);
        s.delete(&a.id).unwrap();
        assert!(s.list().iter().all(|n| n.id != a.id));
        let s2 = Store::open(tmp.path()).unwrap();
        assert!(s2.get_meta(&a.id).unwrap().deleted);
    }

    #[test]
    fn tag_parsing() {
        assert_eq!(tags_of("# Heading\nsee #Idea and #todo/later, not C# or a#b or #2024"), vec!["idea", "todo/later"]);
        assert_eq!(with_tags("hi #idea", &["idea".into(), "x".into(), "bad tag".into()]), "hi #idea\n\n#x");
        assert_eq!(title_of("#work #idea\nReal title"), "Real title");
        assert_eq!(title_of("Call the landlord #home"), "Call the landlord");
        assert_eq!(title_of("# Heading stays"), "Heading stays");
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
