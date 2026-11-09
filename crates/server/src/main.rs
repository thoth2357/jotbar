//! jotbar sync server: a tiny token-protected note store.
//!
//! Environment:
//!   JOTBAR_TOKEN  required; clients send it as `Authorization: Bearer <token>`
//!   JOTBAR_DATA   data directory (default ./data)
//!   JOTBAR_ADDR   listen address (default 127.0.0.1:8787)
//!
//! Notes are stored as one plain Markdown file per note (same format as the
//! folder sync target), so a backup is just a copy of the directory.

use anyhow::{bail, Context, Result};
use axum::{
    extract::{Path, Request, State},
    http::{header, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use jotbar_core::remote::folder::{FolderBackend, Put};
use jotbar_core::{valid_id, PutNote, RemoteMeta, RemoteNote};
use std::{path::PathBuf, sync::Arc};
use tokio::sync::Mutex;

struct AppState {
    store: FolderBackend,
    token: String,
    /// Serialises writes so the compare-and-swap in PUT is atomic.
    write_lock: Mutex<()>,
}

type Shared = Arc<AppState>;

fn app(state: Shared) -> Router {
    let api = Router::new()
        .route("/api/notes", get(list))
        .route("/api/notes/{id}", get(get_note).put(put_note))
        .route_layer(middleware::from_fn_with_state(state.clone(), auth));
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .merge(api)
        .layer(tower_http::limit::RequestBodyLimitLayer::new(16 * 1024 * 1024))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state)
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn auth(State(s): State<Shared>, req: Request, next: Next) -> Response {
    let ok = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|t| ct_eq(t.as_bytes(), s.token.as_bytes()));
    if ok {
        next.run(req).await
    } else {
        (StatusCode::UNAUTHORIZED, "unauthorized").into_response()
    }
}

struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, self.1).into_response()
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        tracing::error!("{e:#}");
        ApiError(StatusCode::INTERNAL_SERVER_ERROR, "internal error".into())
    }
}

fn check_id(id: &str) -> Result<(), ApiError> {
    if valid_id(id) {
        Ok(())
    } else {
        Err(ApiError(StatusCode::BAD_REQUEST, "invalid id".into()))
    }
}

async fn list(State(s): State<Shared>) -> Result<Json<Vec<RemoteMeta>>, ApiError> {
    Ok(Json(s.store.list()?))
}

async fn get_note(State(s): State<Shared>, Path(id): Path<String>) -> Result<Json<RemoteNote>, ApiError> {
    check_id(&id)?;
    s.store
        .get(&id)?
        .map(Json)
        .ok_or(ApiError(StatusCode::NOT_FOUND, "not found".into()))
}

async fn put_note(
    State(s): State<Shared>,
    Path(id): Path<String>,
    Json(body): Json<PutNote>,
) -> Result<Response, ApiError> {
    check_id(&id)?;
    let _guard = s.write_lock.lock().await;
    Ok(match s.store.put(&id, &body)? {
        Put::Written(meta) => Json(meta).into_response(),
        Put::Conflict(cur) => (StatusCode::CONFLICT, Json(cur)).into_response(),
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let token = std::env::var("JOTBAR_TOKEN").unwrap_or_default();
    if token.len() < 16 {
        bail!("set JOTBAR_TOKEN to a secret of at least 16 characters (e.g. `openssl rand -hex 32`)");
    }
    let dir = PathBuf::from(std::env::var("JOTBAR_DATA").unwrap_or_else(|_| "data".into()));
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let addr = std::env::var("JOTBAR_ADDR").unwrap_or_else(|_| "127.0.0.1:8787".into());
    let state = Arc::new(AppState { store: FolderBackend::new(dir.clone())?, token, write_lock: Mutex::new(()) });
    let listener = tokio::net::TcpListener::bind(&addr).await.with_context(|| format!("binding {addr}"))?;
    tracing::info!("jotbar-server listening on {addr}, data in {}", dir.display());
    axum::serve(listener, app(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use jotbar_core::remote::{Remote, Target};
    use jotbar_core::{sync::sync, Store};
    use std::sync::Mutex as StdMutex;

    async fn spawn_server() -> (String, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let state = Arc::new(AppState {
            store: FolderBackend::new(dir.path().to_path_buf()).unwrap(),
            token: "test-token-1234567890".into(),
            write_lock: Mutex::new(()),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app(state)).await.unwrap() });
        (url, dir)
    }

    fn contents(s: &StdMutex<Store>) -> Vec<String> {
        let s = s.lock().unwrap();
        let mut v: Vec<_> = s.list().iter().map(|m| s.read(&m.id).unwrap()).collect();
        v.sort();
        v
    }

    #[tokio::test]
    async fn two_devices_sync_and_resolve_conflicts() {
        let (url, _srv) = spawn_server().await;
        let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let a = StdMutex::new(Store::open(da.path()).unwrap());
        let b = StdMutex::new(Store::open(db.path()).unwrap());
        let mut client = Remote::connect(&Target::Server { url: url.clone(), token: "test-token-1234567890".into() }).unwrap();

        // Bad token is rejected.
        let mut bad = Remote::connect(&Target::Server { url: url.clone(), token: "wrong".into() }).unwrap();
        assert!(sync(&a, &mut bad).await.is_err());

        // A writes, B receives. Blank starter notes are not pushed.
        let id = a.lock().unwrap().create("shopping\nmilk").unwrap().id;
        sync(&a, &mut client).await.unwrap();
        let r = sync(&b, &mut client).await.unwrap();
        assert_eq!(r.pulled, 1);
        assert!(contents(&b).contains(&"shopping\nmilk".to_string()));

        // B edits, A receives.
        b.lock().unwrap().write(&id, "shopping\nmilk\neggs").unwrap();
        sync(&b, &mut client).await.unwrap();
        sync(&a, &mut client).await.unwrap();
        assert_eq!(a.lock().unwrap().read(&id).unwrap(), "shopping\nmilk\neggs");

        // Both edit the same note: server version wins in place, local edit
        // is kept as a conflicted copy.
        a.lock().unwrap().write(&id, "A version").unwrap();
        b.lock().unwrap().write(&id, "B version").unwrap();
        sync(&a, &mut client).await.unwrap();
        let r = sync(&b, &mut client).await.unwrap();
        assert_eq!(r.conflicts, 1);
        assert_eq!(b.lock().unwrap().read(&id).unwrap(), "A version");
        assert!(contents(&b).iter().any(|c| c.ends_with("B version") && c.contains("Conflicted")));

        // The conflict copy propagates back to A.
        sync(&b, &mut client).await.unwrap();
        sync(&a, &mut client).await.unwrap();
        assert!(contents(&a).iter().any(|c| c.ends_with("B version")));

        // Deletion propagates.
        a.lock().unwrap().delete(&id).unwrap();
        sync(&a, &mut client).await.unwrap();
        sync(&b, &mut client).await.unwrap();
        assert!(b.lock().unwrap().list().iter().all(|m| m.id != id));

        // Both converge.
        sync(&a, &mut client).await.unwrap();
        sync(&b, &mut client).await.unwrap();
        assert_eq!(
            contents(&a).into_iter().filter(|c| !c.is_empty()).collect::<Vec<_>>(),
            contents(&b).into_iter().filter(|c| !c.is_empty()).collect::<Vec<_>>()
        );
    }
}
