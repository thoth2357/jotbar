//! Target: a self-hosted `jotbar-server`.

use super::PutResult;
use crate::{PutNote, RemoteMeta, RemoteNote};
use anyhow::{bail, Result};
use reqwest::StatusCode;
use std::time::Duration;

pub struct ServerBackend {
    http: reqwest::Client,
    base: String,
    token: String,
}

pub(crate) fn http_client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent(concat!("jotbar/", env!("CARGO_PKG_VERSION")))
        .build()?)
}

pub(crate) fn check_url(url: &str) -> Result<String> {
    let base = url.trim().trim_end_matches('/').to_string();
    if !(base.starts_with("http://") || base.starts_with("https://")) {
        bail!("URL must start with http:// or https://");
    }
    Ok(base)
}

impl ServerBackend {
    pub fn new(url: &str, token: &str) -> Result<Self> {
        Ok(ServerBackend { http: http_client()?, base: check_url(url)?, token: token.to_string() })
    }

    fn url(&self, path: &str) -> String {
        format!("{}/api/notes{}", self.base, path)
    }

    pub async fn list(&self) -> Result<Vec<RemoteMeta>> {
        let r = self.http.get(self.url("")).bearer_auth(&self.token).send().await?;
        if r.status() == StatusCode::UNAUTHORIZED {
            bail!("server rejected the token");
        }
        Ok(r.error_for_status()?.json().await?)
    }

    pub async fn get(&self, id: &str) -> Result<Option<RemoteNote>> {
        let r = self.http.get(self.url(&format!("/{id}"))).bearer_auth(&self.token).send().await?;
        if r.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Ok(Some(r.error_for_status()?.json().await?))
    }

    pub async fn put(&self, id: &str, body: &PutNote) -> Result<PutResult> {
        let r = self
            .http
            .put(self.url(&format!("/{id}")))
            .bearer_auth(&self.token)
            .json(body)
            .send()
            .await?;
        if r.status() == StatusCode::CONFLICT {
            return Ok(PutResult::Conflict);
        }
        r.error_for_status()?;
        Ok(PutResult::Ok)
    }
}
