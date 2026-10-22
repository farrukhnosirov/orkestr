//! HTTP client for the sync server (`server.rs`), the transport of the
//! `RemoteBackend` (self-hosted / cloud storage modes, §10).

use std::time::Duration;

use reqwest::StatusCode;

use crate::server::{CasReq, HeadDoc};
use crate::SyncError;

/// Outcome of a compare-and-swap on a head.
#[derive(Debug, Clone)]
pub enum CasOutcome {
    /// Updated; new etag.
    Updated(String),
    /// Someone else moved the head (current value, if any).
    Conflict(Option<HeadDoc>),
}

/// HTTP client of one sync server account.
#[derive(Debug, Clone)]
pub struct HttpRemote {
    base: String,
    token: String,
    http: reqwest::Client,
}

fn net(e: reqwest::Error) -> SyncError {
    SyncError::Network(e.to_string())
}

async fn check(resp: reqwest::Response) -> Result<reqwest::Response, SyncError> {
    match resp.status() {
        s if s.is_success() => Ok(resp),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            Err(SyncError::Network("sync server rejected the token".into()))
        }
        StatusCode::NOT_FOUND => Err(SyncError::NotFound),
        StatusCode::TOO_MANY_REQUESTS => Err(SyncError::Network("rate limited".into())),
        s => Err(SyncError::Network(format!("sync server answered {s}"))),
    }
}

impl HttpRemote {
    /// Client for `server_url` (the gateway; `/v1/sync` is appended unless present).
    ///
    /// # Errors
    /// `Config` when the URL is not http(s).
    pub fn new(server_url: &str, token: &str) -> Result<Self, SyncError> {
        let url = server_url.trim_end_matches('/');
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(SyncError::Config("server URL must start with http:// or https://".into()));
        }
        let base = if url.ends_with("/v1/sync") { url.to_owned() } else { format!("{url}/v1/sync") };
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| SyncError::Config(e.to_string()))?;
        Ok(Self { base, token: token.to_owned(), http })
    }

    fn req(&self, m: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http.request(m, format!("{}{path}", self.base)).bearer_auth(&self.token)
    }

    /// Which chunk ids the server already has.
    ///
    /// # Errors
    /// Network failures.
    pub async fn has_chunks(&self, ids: &[String]) -> Result<Vec<bool>, SyncError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        #[derive(serde::Deserialize)]
        struct Resp {
            have: Vec<bool>,
        }
        let r = self
            .req(reqwest::Method::POST, "/chunks/has")
            .json(&serde_json::json!({ "ids": ids }))
            .send()
            .await
            .map_err(net)?;
        Ok(check(r).await?.json::<Resp>().await.map_err(net)?.have)
    }

    /// Uploads an object (`chunks` / `manifests`).
    ///
    /// # Errors
    /// Network failures.
    pub async fn put_object(&self, kind: &str, id: &str, body: Vec<u8>) -> Result<(), SyncError> {
        let r = self.req(reqwest::Method::PUT, &format!("/{kind}/{id}")).body(body).send().await.map_err(net)?;
        check(r).await.map(|_| ())
    }

    /// Downloads an object.
    ///
    /// # Errors
    /// `NotFound` / network failures.
    pub async fn get_object(&self, kind: &str, id: &str) -> Result<Vec<u8>, SyncError> {
        let r = self.req(reqwest::Method::GET, &format!("/{kind}/{id}")).send().await.map_err(net)?;
        Ok(check(r).await?.bytes().await.map_err(net)?.to_vec())
    }

    /// Current head of a stream.
    ///
    /// # Errors
    /// Network failures.
    pub async fn get_head(&self, stream: &str) -> Result<Option<HeadDoc>, SyncError> {
        let r = self.req(reqwest::Method::GET, &format!("/heads/{stream}")).send().await.map_err(net)?;
        match check(r).await {
            Ok(r) => Ok(Some(r.json().await.map_err(net)?)),
            Err(SyncError::NotFound) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Compare-and-swap of a head.
    ///
    /// # Errors
    /// Network failures.
    pub async fn cas_head(&self, stream: &str, expected: Option<&str>, value: String) -> Result<CasOutcome, SyncError> {
        let body = CasReq { expected: expected.map(str::to_owned), value };
        let r = self.req(reqwest::Method::PUT, &format!("/heads/{stream}")).json(&body).send().await.map_err(net)?;
        if r.status() == StatusCode::PRECONDITION_FAILED {
            let cur = r.json::<HeadDoc>().await.ok();
            return Ok(CasOutcome::Conflict(cur));
        }
        #[derive(serde::Deserialize)]
        struct Resp {
            etag: String,
        }
        Ok(CasOutcome::Updated(check(r).await?.json::<Resp>().await.map_err(net)?.etag))
    }
}
