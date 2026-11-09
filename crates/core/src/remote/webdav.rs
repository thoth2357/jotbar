//! WebDAV target (Nextcloud, ownCloud, Synology, Fastmail, rclone serve …).
//!
//! Layout inside the configured collection:
//!   manifest.json   { "<id>": RemoteMeta, … }  -- one GET lists everything
//!   <id>.<hash16>.md  note file (same format as the folder target)
//!
//! Note files are content-addressed, so writing one never clobbers another
//! device's version. The manifest is then updated with `If-Match: <etag>`;
//! if another device changed it in between, the server answers 412 and we
//! re-read and re-check, so concurrent devices can't lose edits.

use super::{decode_note, encode_note, PutResult};
use crate::{PutNote, RemoteMeta, RemoteNote};
use anyhow::{bail, Context, Result};
use reqwest::{header, Method, StatusCode};
use std::collections::BTreeMap;

pub struct WebdavBackend {
    http: reqwest::Client,
    base: String,
    user: String,
    pass: String,
    manifest: BTreeMap<String, RemoteMeta>,
    etag: Option<String>,
}

impl WebdavBackend {
    pub fn new(url: &str, user: &str, pass: &str) -> Result<Self> {
        Ok(WebdavBackend {
            http: super::server::http_client()?,
            base: super::server::check_url(url)?,
            user: user.into(),
            pass: pass.into(),
            manifest: BTreeMap::new(),
            etag: None,
        })
    }

    fn req(&self, m: Method, name: &str) -> reqwest::RequestBuilder {
        self.http
            .request(m, format!("{}/{name}", self.base))
            .basic_auth(&self.user, Some(&self.pass))
    }

    async fn ensure_collection(&self) -> Result<()> {
        let r = self.req(Method::from_bytes(b"MKCOL")?, "").send().await?;
        // 201 created, 405 already exists
        if !(r.status().is_success() || r.status() == StatusCode::METHOD_NOT_ALLOWED) {
            bail!("could not create WebDAV folder: HTTP {}", r.status());
        }
        Ok(())
    }

    async fn load_manifest(&mut self) -> Result<()> {
        let r = self.req(Method::GET, "manifest.json").send().await?;
        match r.status() {
            StatusCode::UNAUTHORIZED => bail!("WebDAV server rejected the username/password"),
            StatusCode::NOT_FOUND | StatusCode::CONFLICT => {
                // 409 = parent collection missing
                self.ensure_collection().await?;
                self.manifest.clear();
                self.etag = None;
            }
            s if s.is_success() => {
                self.etag = r.headers().get(header::ETAG).and_then(|v| v.to_str().ok()).map(String::from);
                self.manifest = serde_json::from_slice(&r.bytes().await?).context("parsing manifest.json")?;
            }
            s => bail!("WebDAV GET manifest.json: HTTP {s}"),
        }
        Ok(())
    }

    /// Write the manifest only if nobody else changed it. Returns false on 412.
    async fn save_manifest(&mut self) -> Result<bool> {
        let mut rq = self
            .req(Method::PUT, "manifest.json")
            .header(header::CONTENT_TYPE, "application/json")
            .body(serde_json::to_vec_pretty(&self.manifest)?);
        rq = match &self.etag {
            Some(e) => rq.header(header::IF_MATCH, e),
            None => rq.header(header::IF_NONE_MATCH, "*"),
        };
        let r = rq.send().await?;
        if r.status() == StatusCode::PRECONDITION_FAILED {
            return Ok(false);
        }
        if !r.status().is_success() {
            bail!("WebDAV PUT manifest.json: HTTP {}", r.status());
        }
        self.etag = r.headers().get(header::ETAG).and_then(|v| v.to_str().ok()).map(String::from);
        if self.etag.is_none() {
            // Some servers don't return an ETag on PUT; fetch it.
            self.load_manifest().await?;
        }
        Ok(true)
    }

    pub async fn list(&mut self) -> Result<Vec<RemoteMeta>> {
        self.load_manifest().await?;
        Ok(self.manifest.values().cloned().collect())
    }

    pub async fn get(&self, id: &str) -> Result<Option<RemoteNote>> {
        let Some(meta) = self.manifest.get(id) else { return Ok(None) };
        let r = self.req(Method::GET, &file_name(id, &meta.hash)).send().await?;
        if r.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let raw = r.error_for_status()?.text().await?;
        Ok(Some(decode_note(id, &raw)))
    }

    pub async fn put(&mut self, id: &str, body: &PutNote) -> Result<PutResult> {
        let hash = crate::content_hash(&body.content, body.deleted);
        let meta = RemoteMeta { id: id.into(), updated_at: body.updated_at, hash: hash.clone(), deleted: body.deleted };
        for _ in 0..5 {
            let cur = self.manifest.get(id).map(|m| m.hash.clone());
            if cur.as_deref() == Some(hash.as_str()) {
                return Ok(PutResult::Ok);
            }
            if cur.is_some() && cur != body.base_hash {
                return Ok(PutResult::Conflict);
            }
            let name = file_name(id, &hash);
            let r = self
                .req(Method::PUT, &name)
                .header(header::CONTENT_TYPE, "text/markdown; charset=utf-8")
                .body(encode_note(&body.content, body.updated_at, body.deleted))
                .send()
                .await?;
            if !r.status().is_success() {
                bail!("WebDAV PUT {name}: HTTP {}", r.status());
            }
            let prev = self.manifest.insert(id.into(), meta.clone());
            if self.save_manifest().await? {
                if let Some(p) = prev {
                    // Best effort: drop the superseded version.
                    let _ = self.req(Method::DELETE, &file_name(id, &p.hash)).send().await;
                }
                return Ok(PutResult::Ok);
            }
            // Someone else updated the manifest; reload and re-check.
            self.load_manifest().await?;
        }
        bail!("WebDAV manifest kept changing; try again")
    }
}

fn file_name(id: &str, hash: &str) -> String {
    format!("{id}.{}.md", &hash[..hash.len().min(16)])
}
