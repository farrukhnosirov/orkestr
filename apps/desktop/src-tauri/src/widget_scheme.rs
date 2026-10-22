//! `orkestr-widget` URI scheme for sandboxed plugin widgets (ADR-0034).
//!
//! A `srcdoc` iframe inherits the app's CSP (no inline scripts), so plugin
//! widget documents are served from this scheme instead, with the widget CSP
//! as their own response header. The webview stores a document with the
//! `widget_document` command and gets a **one-time** URL back
//! (`http://orkestr-widget.localhost/<token>` on Windows/Android,
//! `orkestr-widget://localhost/<token>` elsewhere); the first request consumes
//! it. The iframe stays `sandbox="allow-scripts"` (opaque origin: no Tauri
//! IPC, no capabilities, no app storage) and the response CSP adds
//! `sandbox allow-scripts` so the document is sandboxed even if it were ever
//! loaded without the attribute. Business data still flows only through the
//! RPC bridge (`postMessage`, validated by the host).

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::http::{header, Request, Response, StatusCode};

/// Scheme name (also in `tauri.conf.json` `frame-src`).
pub const SCHEME: &str = "orkestr-widget";
/// Max stored documents (oldest evicted first).
const MAX_DOCS: usize = 64;
/// Unused documents expire after this long.
const TTL: Duration = Duration::from_secs(120);
/// Max document size (core caps widget HTML at 512 KB; the host adds a CSP meta).
pub const MAX_DOC_BYTES: usize = 512 * 1024 + 4096;

/// Response CSP: the core's widget CSP plus a forced sandbox.
#[must_use]
pub fn response_csp() -> String {
    format!("{}; sandbox allow-scripts", orkestr_core::rpc::widgets::WIDGET_CSP)
}

/// Stored one-time widget documents.
#[derive(Default)]
pub struct WidgetDocs {
    docs: Mutex<HashMap<String, (String, Instant, u64)>>,
    seq: std::sync::atomic::AtomicU64,
}

impl WidgetDocs {
    /// Stores `html` and returns its token.
    ///
    /// # Errors
    /// Document too large.
    pub fn put(&self, html: String) -> Result<String, String> {
        if html.len() > MAX_DOC_BYTES {
            return Err(format!("widget document larger than {MAX_DOC_BYTES} bytes"));
        }
        let token = orkestr_core::auth::generate_token();
        let mut docs = self.docs.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = Instant::now();
        docs.retain(|_, (_, at, _)| now.duration_since(*at) < TTL);
        while docs.len() >= MAX_DOCS {
            let Some(oldest) = docs.iter().min_by_key(|(_, (_, _, n))| *n).map(|(k, _)| k.clone()) else { break };
            docs.remove(&oldest);
        }
        let n = self.seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        docs.insert(token.clone(), (html, now, n));
        Ok(token)
    }

    /// Takes (consumes) the document of `token`.
    pub fn take(&self, token: &str) -> Option<String> {
        let mut docs = self.docs.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let (html, at, _) = docs.remove(token)?;
        (at.elapsed() < TTL).then_some(html)
    }
}

/// URL of a stored document for this platform.
#[must_use]
pub fn url_for(token: &str) -> String {
    if cfg!(any(windows, target_os = "android")) {
        format!("http://{SCHEME}.localhost/{token}")
    } else {
        format!("{SCHEME}://localhost/{token}")
    }
}

/// Serves one request of the scheme.
#[must_use]
pub fn serve(docs: &WidgetDocs, request: &Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
    let token = request.uri().path().trim_start_matches('/');
    let found = (request.method() == tauri::http::Method::GET && !token.is_empty() && !token.contains('/'))
        .then(|| docs.take(token))
        .flatten();
    let builder = Response::builder()
        .header(header::CONTENT_SECURITY_POLICY, response_csp())
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::REFERRER_POLICY, "no-referrer");
    let response = match found {
        Some(html) => builder
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
            .body(Cow::Owned(html.into_bytes())),
        None => builder
            .status(StatusCode::NOT_FOUND)
            .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
            .body(Cow::Borrowed(&b"widget document not found"[..])),
    };
    response.unwrap_or_else(|_| Response::new(Cow::Borrowed(&b""[..])))
}

/// Stores a widget document and returns its one-time URL.
#[tauri::command]
pub fn widget_document(docs: tauri::State<'_, WidgetDocs>, html: String) -> Result<String, String> {
    docs.put(html).map(|t| url_for(&t))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(path: &str) -> Request<Vec<u8>> {
        Request::builder().uri(format!("http://orkestr-widget.localhost{path}")).body(Vec::new()).unwrap()
    }

    #[test]
    fn documents_are_served_once_with_the_widget_csp() {
        let docs = WidgetDocs::default();
        let token = docs.put("<p>hi</p>".into()).unwrap();
        let r = serve(&docs, &get(&format!("/{token}")));
        assert_eq!(r.status(), StatusCode::OK);
        let csp = r.headers()[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
        assert!(csp.contains("connect-src 'none'") && csp.ends_with("sandbox allow-scripts"));
        assert_eq!(r.body().as_ref(), b"<p>hi</p>");
        // One-time.
        assert_eq!(serve(&docs, &get(&format!("/{token}"))).status(), StatusCode::NOT_FOUND);
        assert_eq!(serve(&docs, &get("/")).status(), StatusCode::NOT_FOUND);
        assert_eq!(serve(&docs, &get("/nope")).status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn oversized_documents_are_refused_and_store_is_bounded() {
        let docs = WidgetDocs::default();
        assert!(docs.put("x".repeat(MAX_DOC_BYTES + 1)).is_err());
        let first = docs.put("a".into()).unwrap();
        for _ in 0..MAX_DOCS {
            docs.put("b".into()).unwrap();
        }
        assert!(docs.take(&first).is_none(), "oldest evicted");
        assert!(url_for("t").ends_with("/t"));
    }
}
