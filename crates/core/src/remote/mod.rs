//! Sync targets. You choose where your notes go; jotbar only needs three
//! operations from a target: list, get and compare-and-swap put.
//!
//! * [`folder`]  any directory: Syncthing, Nextcloud/Dropbox desktop folder,
//!   a NAS mount, a USB stick. No server needed.
//! * [`server`]  the bundled `jotbar-server`, self-hosted anywhere.
//! * [`webdav`]  Nextcloud, ownCloud, Fastmail files, any WebDAV share.

pub mod folder;
#[cfg(feature = "client")]
pub mod server;
#[cfg(feature = "client")]
pub mod webdav;

use crate::{RemoteMeta, RemoteNote};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

/// Result of a compare-and-swap write.
pub enum PutResult {
    Ok,
    /// The target's copy changed since `base_hash`; nothing was written.
    Conflict,
}

/// User-facing description of a sync target, as stored in the app config.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Target {
    Folder { path: String },
    Server { url: String, token: String },
    Webdav { url: String, username: String, password: String },
}

impl Target {
    pub fn label(&self) -> String {
        match self {
            Target::Folder { path } => format!("folder {path}"),
            Target::Server { url, .. } => format!("server {url}"),
            Target::Webdav { url, .. } => format!("WebDAV {url}"),
        }
    }
}

/// A connected target. Enum dispatch keeps the async methods simple.
pub enum Remote {
    Folder(folder::FolderBackend),
    #[cfg(feature = "client")]
    Server(server::ServerBackend),
    #[cfg(feature = "client")]
    Webdav(webdav::WebdavBackend),
}

impl Remote {
    pub fn connect(t: &Target) -> Result<Remote> {
        Ok(match t {
            Target::Folder { path } => Remote::Folder(folder::FolderBackend::new(expand_home(path))?),
            #[cfg(feature = "client")]
            Target::Server { url, token } => Remote::Server(server::ServerBackend::new(url, token)?),
            #[cfg(feature = "client")]
            Target::Webdav { url, username, password } => {
                Remote::Webdav(webdav::WebdavBackend::new(url, username, password)?)
            }
            #[allow(unreachable_patterns)]
            _ => bail!("this build has no network sync support"),
        })
    }

    pub async fn list(&mut self) -> Result<Vec<RemoteMeta>> {
        match self {
            Remote::Folder(b) => b.list(),
            #[cfg(feature = "client")]
            Remote::Server(b) => b.list().await,
            #[cfg(feature = "client")]
            Remote::Webdav(b) => b.list().await,
        }
    }

    pub async fn get(&mut self, id: &str) -> Result<RemoteNote> {
        let n = match self {
            Remote::Folder(b) => b.get(id)?,
            #[cfg(feature = "client")]
            Remote::Server(b) => b.get(id).await?,
            #[cfg(feature = "client")]
            Remote::Webdav(b) => b.get(id).await?,
        };
        n.ok_or_else(|| anyhow::anyhow!("note {id} vanished from the sync target"))
    }

    pub async fn put(&mut self, id: &str, body: &crate::PutNote) -> Result<PutResult> {
        match self {
            Remote::Folder(b) => b.put(id, body).map(|r| match r {
                folder::Put::Written(_) => PutResult::Ok,
                folder::Put::Conflict(_) => PutResult::Conflict,
            }),
            #[cfg(feature = "client")]
            Remote::Server(b) => b.put(id, body).await,
            #[cfg(feature = "client")]
            Remote::Webdav(b) => b.put(id, body).await,
        }
    }
}

pub fn expand_home(p: &str) -> std::path::PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => std::env::var_os("HOME")
            .map(|h| std::path::PathBuf::from(h).join(rest))
            .unwrap_or_else(|| p.into()),
        None => p.into(),
    }
}

// ---- on-disk note file format shared by the folder, server and WebDAV targets ----
//
// One self-contained file per note, so file-sync tools (Syncthing etc.) can
// never deliver content and metadata out of step:
//
//   <!-- jotbar:{"created_at":1731150000000,"updated_at":1731150000000,"deleted":false} -->
//   the note text…

#[derive(Serialize, Deserialize)]
struct Header {
    #[serde(default)]
    created_at: i64,
    updated_at: i64,
    deleted: bool,
}

const PREFIX: &str = "<!-- jotbar:";
const SUFFIX: &str = " -->";

pub fn encode_note(content: &str, created_at: i64, updated_at: i64, deleted: bool) -> String {
    let h = serde_json::to_string(&Header { created_at, updated_at, deleted }).expect("header serialises");
    let body = if deleted { "" } else { content };
    format!("{PREFIX}{h}{SUFFIX}\n{body}")
}

/// Parse a note file. Files without a header (dropped in by hand) are treated
/// as live notes with timestamp 0.
pub fn decode_note(id: &str, raw: &str) -> RemoteNote {
    let parsed = raw.split_once('\n').and_then(|(first, rest)| {
        let json = first.strip_prefix(PREFIX)?.strip_suffix(SUFFIX)?;
        let h: Header = serde_json::from_str(json).ok()?;
        Some((h, rest))
    });
    let (created_at, updated_at, deleted, content) = match parsed {
        Some((h, rest)) => (h.created_at, h.updated_at, h.deleted, if h.deleted { "" } else { rest }),
        None => (0, 0, false, raw),
    };
    RemoteNote {
        id: id.to_string(),
        content: content.to_string(),
        created_at,
        updated_at,
        hash: crate::content_hash(content, deleted),
        deleted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_format_roundtrip() {
        let raw = encode_note("hi\nthere", 4, 5, false);
        let n = decode_note("x", &raw);
        assert_eq!((n.content.as_str(), n.created_at, n.updated_at, n.deleted), ("hi\nthere", 4, 5, false));
        let d = decode_note("x", &encode_note("gone", 4, 6, true));
        assert!(d.deleted && d.content.is_empty());
        assert_eq!(decode_note("x", "plain").content, "plain");
    }

    #[test]
    fn target_config_shape() {
        let t: Target = serde_json::from_str(r#"{"type":"folder","path":"~/Sync/notes"}"#).unwrap();
        assert_eq!(t, Target::Folder { path: "~/Sync/notes".into() });
    }
}
