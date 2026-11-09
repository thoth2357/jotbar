//! Wire types shared by the desktop client and the sync server.

use serde::{Deserialize, Serialize};

/// Summary of one note as the server knows it (`GET /api/notes`).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RemoteMeta {
    pub id: String,
    pub updated_at: i64,
    pub hash: String,
    pub deleted: bool,
}

/// Full note (`GET /api/notes/{id}`, and the 409 body of a rejected PUT).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoteNote {
    pub id: String,
    pub content: String,
    #[serde(default)]
    pub created_at: i64,
    pub updated_at: i64,
    pub hash: String,
    pub deleted: bool,
}

/// Body of `PUT /api/notes/{id}`.
///
/// `base_hash` is the server hash the client last saw. The server refuses the
/// write with 409 if its copy has moved on since then, so nothing is overwritten
/// blindly.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PutNote {
    pub content: String,
    #[serde(default)]
    pub created_at: i64,
    pub updated_at: i64,
    pub deleted: bool,
    pub base_hash: Option<String>,
}
