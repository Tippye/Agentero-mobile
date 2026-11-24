//! Backend-agnostic storage contract (Strategy): every backend implements
//! [`RemoteStore`], the engine is generic over it, and [`SyncStore`] is the
//! single composition point that picks a concrete client from the config at
//! runtime. Adding a backend = one new client module implementing the trait
//! + one enum variant.
//!
//! The engine-facing surface is deliberately narrow: `get` (with etag) and
//! conditional `put` carry the whole CAS protocol; `ensure_root` and the
//! conditional-write probe exist for `sync_configure`'s connection test.

use crate::error::AppError;
use crate::features::sync::agentero::AgenteroClient;
use crate::features::sync::config::{SyncBackendConfig, SyncBackendKind};
use crate::features::sync::s3::S3Client;
use crate::features::sync::webdav::WebdavClient;
use crate::features::sync::SYNC_UNREACHABLE;
use std::future::Future;
use std::path::Path;

pub enum PutCondition {
    /// Create-only (`If-None-Match: *`).
    IfNoneMatch,
    /// Replace-only when unchanged (`If-Match: <etag>`).
    IfMatch(String),
    /// Plain PUT, no precondition. Only the opt-in degraded protocol may use
    /// it, and only for keys whose safety does not rest on CAS (the HEAD
    /// publish under the publish lock, and first-contact `vault.json` with
    /// read-back verification). Content-addressed blobs and unique-key
    /// manifests keep `IfNoneMatch` even in degraded mode: ignoring servers
    /// treat it as a plain PUT, enforcing servers save re-uploads.
    Unconditional,
}

impl PutCondition {
    pub(super) fn header(self) -> Result<Option<(&'static str, String)>, AppError> {
        match self {
            Self::Unconditional => Ok(None),
            Self::IfNoneMatch => Ok(Some(("If-None-Match", "*".into()))),
            Self::IfMatch(tag)
                if tag
                    .strip_prefix('"')
                    .and_then(|value| value.strip_suffix('"'))
                    .is_some_and(|opaque| {
                        opaque
                            .bytes()
                            .all(|byte| byte == 0x21 || (0x23..=0x7e).contains(&byte))
                    }) =>
            {
                Ok(Some(("If-Match", tag)))
            }
            Self::IfMatch(_) => Err(AppError::message("remote did not supply a strong ETag")),
        }
    }
}

/// Outcome of a degraded-mode publish-lock attempt.
#[derive(Debug, PartialEq, Eq)]
pub enum LockOutcome {
    /// This device holds the lock and may publish HEAD.
    Acquired,
    /// Another device holds a live lock; back off and retry.
    Busy,
    /// The backend has no publish-lock implementation (only WebDAV does in
    /// phase 1); degraded sync must not run against it.
    Unsupported,
}

pub(super) async fn probe_conditions(
    store: &impl RemoteStore,
    key: &str,
) -> Result<bool, AppError> {
    // The safety-critical guarantee is the If-Match CAS on HEAD. Create the
    // probe object (If-None-Match is honored or ignored — both create it),
    // then verify a current-etag replace succeeds and a stale-etag replace
    // is refused. Ignoring If-None-Match alone (Nutstore) is harmless: blobs
    // are content-addressed and manifest keys are unique random, so the
    // create-only puts degrade to idempotent plain PUTs without losing CAS.
    if store.put(key, vec![1], PutCondition::IfNoneMatch).await? != PutOutcome::Ok {
        return Ok(false);
    }
    let Some((_, tag)) = store.get(key).await? else {
        return Ok(false);
    };
    // A weak or missing ETag cannot express If-Match at all. That is the
    // same verdict as a server that ignores it (conditional publication is
    // unavailable), so the degraded opt-in stays reachable instead of the
    // pass failing on a header that cannot be built.
    if PutCondition::IfMatch(tag.clone()).header().is_err() {
        return Ok(false);
    }
    if store
        .put(key, vec![2], PutCondition::IfMatch(tag.clone()))
        .await?
        != PutOutcome::Ok
    {
        return Ok(false);
    }
    Ok(
        store.put(key, vec![3], PutCondition::IfMatch(tag)).await?
            == PutOutcome::PreconditionFailed,
    )
}

#[derive(Debug, PartialEq, Eq)]
pub enum PutOutcome {
    Ok,
    /// The conditional write lost a race (412) — caller decides how to retry.
    PreconditionFailed,
}

/// The storage operations the sync engine relies on. Futures are `Send` so
/// generic engine functions stay usable from Tauri commands.
pub trait RemoteStore: Send + Sync {
    /// Auth-selected stable namespace when it cannot be inferred from config
    /// (official cloud accounts). Never use the rotating bearer as identity.
    fn namespace_identity(&self) -> impl Future<Output = Result<Option<String>, AppError>> + Send {
        async { Ok(None) }
    }
    /// Verify credentials and that the store root is usable; backends that
    /// can (WebDAV) create the configured root when missing.
    fn ensure_root(&self) -> impl Future<Output = Result<(), AppError>> + Send;
    /// GET an object. `None` on 404; otherwise `(body, etag)`.
    fn get(
        &self,
        key: &str,
    ) -> impl Future<Output = Result<Option<(Vec<u8>, String)>, AppError>> + Send;
    fn put(
        &self,
        key: &str,
        body: Vec<u8>,
        condition: PutCondition,
    ) -> impl Future<Output = Result<PutOutcome, AppError>> + Send;
    /// Connection-test probe: whether conditional PUTs actually enforce. A
    /// store whose GET carries no strong ETag cannot express `If-Match`, so
    /// it reports `false` rather than failing.
    fn probe_conditional_writes(&self) -> impl Future<Output = Result<bool, AppError>> + Send;
    /// Degraded-mode publish lock: try to become the sole HEAD publisher.
    /// Default: unsupported. Only WebDAV implements it (phase 1); the engine
    /// refuses degraded sync on backends that answer `Unsupported`.
    fn try_acquire_publish_lock(
        &self,
        _device_id: &str,
    ) -> impl Future<Output = Result<LockOutcome, AppError>> + Send {
        async { Ok(LockOutcome::Unsupported) }
    }
    /// Best-effort release of this device's publish lock. TTL covers crashes;
    /// a failed release must never fail the pass that already published.
    fn release_publish_lock(
        &self,
        _device_id: &str,
    ) -> impl Future<Output = Result<(), AppError>> + Send {
        async { Ok(()) }
    }
}

/// How long a publish lock stays valid without a release. The hold window is
/// only a few requests (re-read HEAD, publish, read back), so no refresh is
/// needed; a holder that exceeds this aborts its pass instead of publishing.
pub(crate) const PUBLISH_LOCK_TTL_SECS: i64 = 90;

/// Runtime selection between the compiled-in backends.
pub enum SyncStore {
    S3(S3Client),
    Webdav(WebdavClient),
    Agentero(AgenteroClient),
}

impl SyncStore {
    pub fn new(cfg: &SyncBackendConfig, vault: &Path) -> Result<Self, AppError> {
        match cfg.backend {
            SyncBackendKind::S3 => Ok(Self::S3(S3Client::new(cfg)?)),
            SyncBackendKind::Webdav => Ok(Self::Webdav(WebdavClient::new(cfg)?)),
            SyncBackendKind::Agentero => Ok(Self::Agentero(AgenteroClient::new(cfg, vault)?)),
            SyncBackendKind::Git => Err(AppError::message(
                "git sync does not use the blob/manifest store",
            )),
        }
    }
}

impl RemoteStore for SyncStore {
    async fn namespace_identity(&self) -> Result<Option<String>, AppError> {
        match self {
            Self::Agentero(client) => client.namespace_identity().await,
            _ => Ok(None),
        }
    }
    async fn ensure_root(&self) -> Result<(), AppError> {
        match self {
            Self::S3(c) => c.ensure_root().await,
            Self::Webdav(c) => c.ensure_root().await,
            Self::Agentero(c) => c.ensure_root().await,
        }
    }

    async fn get(&self, key: &str) -> Result<Option<(Vec<u8>, String)>, AppError> {
        match self {
            Self::S3(c) => c.get(key).await,
            Self::Webdav(c) => c.get(key).await,
            Self::Agentero(c) => c.get(key).await,
        }
    }

    async fn put(
        &self,
        key: &str,
        body: Vec<u8>,
        condition: PutCondition,
    ) -> Result<PutOutcome, AppError> {
        match self {
            Self::S3(c) => c.put(key, body, condition).await,
            Self::Webdav(c) => c.put(key, body, condition).await,
            Self::Agentero(c) => c.put(key, body, condition).await,
        }
    }

    async fn probe_conditional_writes(&self) -> Result<bool, AppError> {
        match self {
            Self::S3(c) => c.probe_conditional_writes().await,
            Self::Webdav(c) => c.probe_conditional_writes().await,
            Self::Agentero(c) => c.probe_conditional_writes().await,
        }
    }

    async fn try_acquire_publish_lock(&self, device_id: &str) -> Result<LockOutcome, AppError> {
        match self {
            Self::Webdav(c) => c.try_acquire_publish_lock(device_id).await,
            _ => Ok(LockOutcome::Unsupported),
        }
    }

    async fn release_publish_lock(&self, device_id: &str) -> Result<(), AppError> {
        match self {
            Self::Webdav(c) => c.release_publish_lock(device_id).await,
            _ => Ok(()),
        }
    }
}

// ---- HTTP plumbing shared by the backend clients -------------------------
//
// One retry loop and one error formatter, so a backend client only decides
// how to build its authenticated request.

/// Never replay a conditional PUT after a transport error: a lost success
/// response followed by a 412 is not evidence that another writer won.
pub(crate) async fn send_with_retries(
    method: &reqwest::Method,
    url: &str,
    build: impl Fn() -> reqwest::RequestBuilder,
) -> Result<reqwest::Response, AppError> {
    let mut last_err = None;
    for attempt in 0..3u32 {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(200 * attempt as u64)).await;
        }
        match build().send().await {
            Ok(resp) => return Ok(resp),
            Err(e) if method != reqwest::Method::PUT && (e.is_connect() || e.is_request()) => {
                log::warn!(
                    target: "agentero::sync",
                    "{method} {url} attempt {}: {e}",
                    attempt + 1
                );
                last_err = Some(e);
            }
            Err(e) => {
                return Err(AppError::message(format!(
                    "{method} {url}: {}",
                    error_chain(&e)
                )))
            }
        }
    }
    let e = last_err.expect("loop sets last_err before exiting");
    let message = format!("{method} {url}: {}", error_chain(&e));
    // Only failures that never reached the server are retryable by a later
    // pass; anything the builder itself rejected would repeat verbatim.
    if is_unreachable(&e) {
        Err(AppError::domain(SYNC_UNREACHABLE, message))
    } else {
        Err(AppError::message(message))
    }
}

/// Whether a transport error means "the server was not reached": DNS lookup
/// failure, refused/reset connection, connect timeout. reqwest funnels DNS
/// through the connect path but does not always flag it as `is_connect()`, so
/// the chain is also inspected — that is the message users hit when a NAS's
/// DDNS name briefly disappears.
fn is_unreachable(error: &reqwest::Error) -> bool {
    error.is_connect() || error.is_timeout() || chain_never_reached_server(&error_chain(error))
}

/// Text markers inside a reqwest source chain that mean the peer was never
/// reached. reqwest reports DNS failures as a generic `Connect`, and their
/// source text is the only part that names the cause the user can act on.
fn chain_never_reached_server(chain: &str) -> bool {
    let chain = chain.to_ascii_lowercase();
    // A rejected certificate repeats on every attempt until the user fixes the
    // server or its CA; it is a configuration error, not a network blip.
    if chain.contains("certificate") || chain.contains("tls handshake") {
        return false;
    }
    [
        "dns error",
        "failed to lookup",
        "connection refused",
        "connection reset",
        "network is unreachable",
        "timed out",
    ]
    .iter()
    .any(|marker| chain.contains(marker))
}

/// Report the HTTP status without leaking provider response bodies to logs or IPC.
pub(crate) async fn check(
    resp: reqwest::Response,
    op: &str,
    key: &str,
) -> Result<reqwest::Response, AppError> {
    if resp.status().is_success() {
        return Ok(resp);
    }
    let status = resp.status();
    Err(AppError::message(format!("{op} {key}: {status}")))
}

/// The entity tag (quoted or bare) as the server spelled it, or "".
pub(crate) fn etag_of(resp: &reqwest::Response) -> String {
    resp.headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

/// reqwest's `Display` stops at the first source; walk the chain so transport
/// failures surface their real cause (connection reset, timeout, …).
pub(crate) fn error_chain(err: &reqwest::Error) -> String {
    let mut out = err.to_string();
    let mut source = std::error::Error::source(err);
    while let Some(s) = source {
        out.push_str(&format!(": {s}"));
        source = s.source();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_failures_that_never_reached_the_server() {
        // Verbatim chain from a NAS whose DDNS name stopped resolving (macOS).
        assert!(chain_never_reached_server(
            "error sending request for url (https://dav.example.net/apps/Agentero/): \
             client error (Connect): dns error: failed to lookup address information: \
             No address associated with hostname: failed to lookup address information: \
             No address associated with hostname"
        ));
        assert!(chain_never_reached_server(
            "tcp connect error: Connection refused (os error 61)"
        ));
        assert!(chain_never_reached_server("operation timed out"));
        // A rejected certificate repeats verbatim until the peer or CA is
        // fixed, so it must not be retried as a network blip.
        assert!(!chain_never_reached_server(
            "invalid peer certificate: UnknownIssuer"
        ));
        // HTTP statuses never take this path; keep the predicate honest anyway.
        assert!(!chain_never_reached_server(
            "PROPFIND apps/Agentero: 401 Unauthorized"
        ));
    }
}
