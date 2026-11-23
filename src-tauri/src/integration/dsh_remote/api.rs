//! Same-origin REST bootstrap. Requests are bounded and never automatically replayed.
use super::{
    crypto::DeviceIdentity,
    protocol::{key, strict_json, MAX_SAFE_INTEGER},
    RemoteError, Result,
};
use reqwest::{Client, Method};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Value};
use std::{collections::HashSet, time::Duration};
use url::{Host, Url};

#[derive(Clone)]
pub struct ServerApi {
    base: Url,
    client: Client,
}

// Intentionally no Debug: account/device bearer credentials are sensitive.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TokenPair {
    pub access_token: String,
    pub access_token_expires_at: u64,
    pub refresh_token: String,
    pub refresh_token_expires_at: u64,
}
impl TokenPair {
    pub fn validate(&self) -> Result<()> {
        if [&self.access_token, &self.refresh_token]
            .iter()
            .any(|s| !(16..=8192).contains(&s.len()))
            || [self.access_token_expires_at, self.refresh_token_expires_at]
                .iter()
                .any(|n| *n == 0 || *n > MAX_SAFE_INTEGER)
        {
            return Err(RemoteError::invalid("Invalid device credential response"));
        }
        Ok(())
    }
}
/// Server-advertised third-party sign-in provider (GitHub, Google, Zhihu, …).
/// Only providers with a QR/browser session are usable by this client.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthProvider {
    pub id: String,
    pub display_name: String,
    pub start_url: String,
    #[serde(default)]
    pub qr_start_url: Option<String>,
}
/// One browser sign-in session. The user authorizes at `scan_url` (open in a
/// browser or scan from a second device); this client polls until completion.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthQrSession {
    pub qr_id: String,
    pub scan_url: String,
    pub expires_in: u64,
    #[serde(default)]
    pub provider: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OAuthQrPoll {
    Pending,
    Expired,
    /// Account (web) bearer token, delivered exactly once by the Server.
    Complete(String),
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OAuthProviderPage {
    items: Vec<OAuthProvider>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OAuthLegacyStatus {
    configured: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountProfile {
    account: String,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostDevice {
    pub device_id: String,
    pub name: String,
    pub role: String,
    pub platform: String,
    pub membership_id: String,
    /// Newer Servers omit presence from the device list and expose it through
    /// `/devices/{id}/presence`; absence is not an offline verdict.
    #[serde(default)]
    pub online: Option<bool>,
    #[serde(default)]
    pub last_seen_at: Option<u64>,
    pub client_version: Option<String>,
    pub harness_version: Option<String>,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthorizedHost {
    #[serde(flatten)]
    pub device: HostDevice,
    pub identity_key: String,
}
/// Authoritative presence from `GET /api/v1/devices/{id}/presence`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DevicePresence {
    pub device_id: String,
    pub online: bool,
    #[serde(default)]
    pub last_seen_at: Option<u64>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DevicePage {
    items: Vec<HostDevice>,
    next_cursor: Option<String>,
}

/// Validate before any request. Public HTTP is rejected even when used by a native client.
pub fn validate_server_url(input: &str) -> Result<Url> {
    let mut url =
        Url::parse(input).map_err(|_| RemoteError::invalid("Invalid Remote server URL"))?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || (url.path() != "/" && !url.path().is_empty())
    {
        return Err(RemoteError::invalid(
            "Remote server URL must be an origin without credentials, path, query or fragment",
        ));
    }
    let private = match url.host() {
        Some(Host::Domain(name)) => {
            name.eq_ignore_ascii_case("localhost") || name.ends_with(".localhost")
        }
        Some(Host::Ipv4(ip)) => ip.is_private() || ip.is_loopback() || ip.is_link_local(),
        Some(Host::Ipv6(ip)) => {
            ip.is_loopback()
                || (ip.segments()[0] & 0xfe00 == 0xfc00)
                || (ip.segments()[0] & 0xffc0 == 0xfe80)
                || ip
                    .to_ipv4_mapped()
                    .is_some_and(|v| v.is_private() || v.is_loopback() || v.is_link_local())
        }
        None => false,
    };
    if url.host().is_none() || !(url.scheme() == "https" || (url.scheme() == "http" && private)) {
        return Err(RemoteError::invalid("Remote server requires HTTPS; HTTP is limited to literal private or loopback addresses"));
    }
    url.set_path("/");
    Ok(url)
}
impl ServerApi {
    pub fn new(origin: &str, timeout: Duration) -> Result<Self> {
        if timeout.is_zero() {
            return Err(RemoteError::invalid("REST timeout must be positive"));
        }
        let client = Client::builder()
            .timeout(timeout)
            .connect_timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| {
                RemoteError::new("CONNECTION_FAILED", "Cannot initialize Remote HTTP client")
            })?;
        Ok(Self {
            base: validate_server_url(origin)?,
            client,
        })
    }
    pub fn websocket_url(&self) -> Result<Url> {
        let mut url = self
            .base
            .join("ws/v1/connect")
            .map_err(|_| RemoteError::invalid("Invalid WebSocket URL"))?;
        let scheme = if self.base.scheme() == "https" {
            "wss"
        } else {
            "ws"
        };
        url.set_scheme(scheme)
            .map_err(|_| RemoteError::invalid("Invalid WebSocket URL"))?;
        Ok(url)
    }
    /// The validated server origin, for account views and same-origin checks.
    pub fn origin(&self) -> String {
        self.base.origin().ascii_serialization()
    }
    /// Only same-origin `/api/v1/auth/` entry paths: a compromised discovery
    /// response must not turn the client into an open request forwarder.
    fn auth_entry_url(&self, path: &str) -> Result<Url> {
        let invalid = || RemoteError::invalid("Invalid OAuth entry path from the Remote server");
        if !path.starts_with("/api/v1/auth/")
            || path
                .chars()
                .any(|c| c.is_control() || c.is_whitespace() || matches!(c, '\\' | '#'))
        {
            return Err(invalid());
        }
        if path
            .split('?')
            .next()
            .unwrap_or(path)
            .split('/')
            .any(|segment| matches!(segment, "." | "..") || segment.contains('%'))
        {
            return Err(invalid());
        }
        let url = self.base.join(path).map_err(|_| invalid())?;
        if url.origin() != self.base.origin() {
            return Err(invalid());
        }
        Ok(url)
    }
    /// Server-defined OAuth providers; legacy per-provider status endpoints are
    /// consulted only when the discovery endpoint itself returns 404.
    pub async fn oauth_providers(&self) -> Result<Vec<OAuthProvider>> {
        let url = self
            .base
            .join("api/v1/auth/oauth/providers")
            .map_err(|_| RemoteError::invalid("Invalid OAuth discovery URL"))?;
        let (status, value) = self.exchange(Method::GET, url, None, None).await?;
        let providers = if status.as_u16() == 404 {
            let mut legacy = Vec::new();
            for (id, name, status_path, start_path) in [
                (
                    "zhihu",
                    "Zhihu",
                    "api/v1/auth/oauth/status",
                    "/api/v1/auth/oauth/start",
                ),
                (
                    "github",
                    "GitHub",
                    "api/v1/auth/oauth/github/status",
                    "/api/v1/auth/oauth/github/start",
                ),
            ] {
                let status_url = self
                    .base
                    .join(status_path)
                    .map_err(|_| RemoteError::invalid("Invalid OAuth status URL"))?;
                let (status, value) = self.exchange(Method::GET, status_url, None, None).await?;
                if status.as_u16() == 404 {
                    continue;
                }
                if !status.is_success() {
                    return Err(protocol_error(&value));
                }
                let configured: OAuthLegacyStatus = serde_json::from_value(value)
                    .map_err(|_| RemoteError::invalid("Invalid OAuth status response"))?;
                if configured.configured {
                    legacy.push(OAuthProvider {
                        id: id.into(),
                        display_name: name.into(),
                        start_url: start_path.into(),
                        qr_start_url: Some(format!("/api/v1/auth/oauth/qr/start?provider={id}")),
                    });
                }
            }
            legacy
        } else {
            if !status.is_success() {
                return Err(protocol_error(&value));
            }
            let page: OAuthProviderPage = serde_json::from_value(value)
                .map_err(|_| RemoteError::invalid("Invalid OAuth providers response"))?;
            page.items
        };
        let mut ids = HashSet::new();
        for provider in &providers {
            let valid_id = provider.id.len() <= 128
                && provider.id.starts_with(|c: char| c.is_ascii_alphanumeric())
                && provider
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
            let name = provider.display_name.trim();
            if !valid_id
                || !ids.insert(provider.id.clone())
                || name.is_empty()
                || provider.display_name.len() > 128
                || provider.display_name.chars().any(char::is_control)
            {
                return Err(RemoteError::invalid("Invalid OAuth provider descriptor"));
            }
            self.auth_entry_url(&provider.start_url)?;
            if let Some(qr) = &provider.qr_start_url {
                self.auth_entry_url(qr)?;
            }
        }
        Ok(providers)
    }
    /// Start one browser sign-in session for a QR-capable provider.
    pub async fn start_oauth_qr(&self, provider: &OAuthProvider) -> Result<OAuthQrSession> {
        let path = provider.qr_start_url.as_deref().ok_or_else(|| {
            RemoteError::new(
                "OAUTH_PROVIDER_UNAVAILABLE",
                "This OAuth provider does not support browser sign-in",
            )
        })?;
        let url = self.auth_entry_url(path)?;
        let session: OAuthQrSession = self
            .request_url(Method::POST, url, None, Some(json!({})))
            .await
            // Starting a sign-in session grants nothing; an interrupted start
            // is safely retryable and must not read as an uncertain write.
            .map_err(|error| RemoteError::new(error.code, error.message))?;
        if session.qr_id.len() < 20
            || session.expires_in == 0
            || session.expires_in > MAX_SAFE_INTEGER
            || session
                .provider
                .as_deref()
                .is_some_and(|id| id != provider.id)
        {
            return Err(RemoteError::invalid("Invalid OAuth sign-in session"));
        }
        let scan = Url::parse(&session.scan_url)
            .map_err(|_| RemoteError::invalid("Invalid OAuth sign-in URL"))?;
        if scan.origin() != self.base.origin() || session.scan_url.chars().any(char::is_control) {
            return Err(RemoteError::invalid(
                "OAuth sign-in URL must stay on the Remote server origin",
            ));
        }
        Ok(session)
    }
    /// Observe one sign-in session. The Server returns the account token exactly
    /// once on completion; an interrupted poll never authorizes a retry loop.
    pub async fn poll_oauth_qr(&self, qr_id: &str) -> Result<OAuthQrPoll> {
        let mut url = self
            .base
            .join("api/v1/auth/oauth/qr/")
            .map_err(|_| RemoteError::invalid("Invalid OAuth poll URL"))?;
        url.path_segments_mut()
            .map_err(|_| RemoteError::invalid("Invalid OAuth poll URL"))?
            .pop_if_empty()
            .push(qr_id);
        let value: Value = self.request_url(Method::GET, url, None, None).await?;
        match value.get("status").and_then(Value::as_str) {
            Some("pending") => Ok(OAuthQrPoll::Pending),
            Some("expired") => Ok(OAuthQrPoll::Expired),
            Some("complete") => {
                let token = value
                    .get("token")
                    .and_then(Value::as_str)
                    .filter(|token| (16..=8192).contains(&token.len()))
                    .ok_or_else(|| RemoteError::invalid("Invalid OAuth sign-in status"))?;
                Ok(OAuthQrPoll::Complete(token.to_owned()))
            }
            _ => Err(RemoteError::invalid("Invalid OAuth sign-in status")),
        }
    }
    /// Account display name for a fresh account (web) token.
    pub async fn account_me(&self, account_token: &str) -> Result<String> {
        let profile: AccountProfile = self
            .request(Method::GET, "api/v1/auth/me", Some(account_token), None)
            .await?;
        if profile.account.is_empty()
            || profile.account.len() > 320
            || profile.account.chars().any(char::is_control)
        {
            return Err(RemoteError::invalid("Invalid account profile response"));
        }
        Ok(profile.account)
    }
    pub async fn register(
        &self,
        identity: &DeviceIdentity,
        name: &str,
        platform: &str,
        version: &str,
        account_token: &str,
    ) -> Result<TokenPair> {
        if uuid::Uuid::parse_str(&identity.device_id).is_err()
            || name.trim().is_empty()
            || name.len() > 128
            || version.is_empty()
            || version.len() > 64
            || platform.is_empty()
            || platform.len() > 64
            || !platform
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
            || !platform.as_bytes()[0].is_ascii_alphanumeric()
        {
            return Err(RemoteError::invalid("Invalid client device descriptor"));
        }
        key(&identity.public_key)?;
        let pair: TokenPair = self.request(Method::POST, "api/v1/devices/register", Some(account_token), Some(json!({"v":1,"device":{"deviceId":identity.device_id,"name":name,"role":"client","platform":platform,"identityKey":identity.public_key,"clientVersion":version}}))).await?;
        pair.validate()?;
        Ok(pair)
    }
    /// Caller must atomically persist the returned pair before using it. An error
    /// preserves the current credentials; HTTP status alone never revokes them.
    pub async fn refresh(&self, device_id: &str, refresh_token: &str) -> Result<TokenPair> {
        let pair: TokenPair = self
            .request(
                Method::POST,
                "api/v1/auth/refresh",
                None,
                Some(json!({"deviceId":device_id,"refreshToken":refresh_token})),
            )
            .await?;
        pair.validate()?;
        Ok(pair)
    }
    pub async fn list_hosts(&self, token: &str) -> Result<Vec<HostDevice>> {
        let mut hosts = Vec::new();
        let mut cursors = HashSet::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut url = self
                .base
                .join("api/v1/devices")
                .map_err(|_| RemoteError::invalid("Invalid device URL"))?;
            url.query_pairs_mut().append_pair("limit", "50");
            if let Some(value) = &cursor {
                url.query_pairs_mut().append_pair("cursor", value);
            }
            let page: DevicePage = self
                .request_url(Method::GET, url, Some(token), None)
                .await?;
            for host in page.items {
                if host.role != "host"
                    || host.membership_id.is_empty()
                    || host.device_id.is_empty()
                    || hosts
                        .iter()
                        .any(|h: &HostDevice| h.device_id == host.device_id)
                {
                    return Err(RemoteError::invalid("Invalid or duplicate authorized Host"));
                }
                hosts.push(host);
            }
            match page.next_cursor {
                None => return Ok(hosts),
                Some(next)
                    if !next.is_empty()
                        && next.len() <= 4096
                        && cursors.insert(next.clone())
                        && cursors.len() < 10000 =>
                {
                    cursor = Some(next)
                }
                _ => {
                    return Err(RemoteError::invalid(
                        "Invalid or repeating Host catalog cursor",
                    ))
                }
            }
        }
    }
    pub async fn host(&self, token: &str, host_id: &str) -> Result<AuthorizedHost> {
        let mut url = self
            .base
            .join("api/v1/devices/")
            .map_err(|_| RemoteError::invalid("Invalid device URL"))?;
        url.path_segments_mut()
            .map_err(|_| RemoteError::invalid("Invalid device URL"))?
            .pop_if_empty()
            .push(host_id);
        let mut host: AuthorizedHost = self
            .request_url(Method::GET, url, Some(token), None)
            .await?;
        if host.device.device_id != host_id
            || host.device.role != "host"
            || host.device.membership_id.is_empty()
        {
            return Err(RemoteError::new(
                "PEER_IDENTITY_MISMATCH",
                "Host descriptor does not match authorized peer",
            ));
        }
        key(&host.identity_key)?;
        // Older deployments inline presence; newer Servers expose it separately.
        // Absence is not an offline verdict: resolve it before any trust decision.
        if host.device.online.is_none() {
            let presence = self.presence(token, host_id).await?;
            host.device.online = Some(presence.online);
            host.device.last_seen_at = presence.last_seen_at;
        }
        Ok(host)
    }
    /// Authoritative online state for one authorized device. A failed lookup
    /// must never be downgraded to an offline Host by the caller.
    pub async fn presence(&self, token: &str, device_id: &str) -> Result<DevicePresence> {
        let mut url = self
            .base
            .join("api/v1/devices/")
            .map_err(|_| RemoteError::invalid("Invalid device URL"))?;
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| RemoteError::invalid("Invalid device URL"))?;
            segments.pop_if_empty().push(device_id).push("presence");
        }
        let presence: DevicePresence = self
            .request_url(Method::GET, url, Some(token), None)
            .await?;
        if presence.device_id != device_id
            || presence
                .last_seen_at
                .is_some_and(|at| at > MAX_SAFE_INTEGER)
        {
            return Err(RemoteError::invalid("Invalid device presence response"));
        }
        Ok(presence)
    }
    async fn request<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> Result<T> {
        let url = self
            .base
            .join(path)
            .map_err(|_| RemoteError::invalid("Invalid API endpoint"))?;
        self.request_url(method, url, token, body).await
    }
    async fn request_url<T: DeserializeOwned>(
        &self,
        method: Method,
        url: Url,
        token: Option<&str>,
        body: Option<Value>,
    ) -> Result<T> {
        let mutation = method != Method::GET;
        let (status, value) = self.exchange(method, url, token, body).await?;
        if !status.is_success() {
            let error = protocol_error(&value);
            return Err(if status.is_server_error() && mutation {
                error.unknown()
            } else {
                error
            });
        }
        serde_json::from_value(value).map_err(|_| {
            let error = RemoteError::invalid("Invalid Remote HTTP response schema");
            if mutation {
                error.unknown()
            } else {
                error
            }
        })
    }
    /// Bounded request/response exchange shared by typed calls and discovery
    /// probes that must observe the HTTP status (404 fallbacks) themselves.
    async fn exchange(
        &self,
        method: Method,
        url: Url,
        token: Option<&str>,
        body: Option<Value>,
    ) -> Result<(reqwest::StatusCode, Value)> {
        let mutation = method != Method::GET;
        let mut request = self
            .client
            .request(method, url)
            .header("Accept", "application/json");
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let mut response = request.send().await.map_err(|_| {
            let error = RemoteError::new(
                "CONNECTION_FAILED",
                "Remote HTTP request failed or timed out",
            );
            if mutation {
                error.unknown()
            } else {
                error
            }
        })?;
        let status = response.status();
        let mut bytes = Vec::new();
        let after_dispatch = |error: RemoteError| if mutation { error.unknown() } else { error };
        while let Some(chunk) = response.chunk().await.map_err(|_| {
            after_dispatch(RemoteError::new(
                "CONNECTION_FAILED",
                "Remote HTTP response interrupted",
            ))
        })? {
            if bytes.len() + chunk.len() > 4 * 1024 * 1024 {
                return Err(after_dispatch(RemoteError::invalid(
                    "Remote HTTP response exceeds limit",
                )));
            }
            bytes.extend_from_slice(&chunk);
        }
        let value = strict_json(&bytes).map_err(after_dispatch)?;
        Ok((status, value))
    }
}

/// Only structured protocol errors carry authoritative auth semantics.
fn protocol_error(value: &Value) -> RemoteError {
    let code = value
        .pointer("/error/code")
        .and_then(Value::as_str)
        .unwrap_or("HTTP_ERROR");
    RemoteError::new(code, "The Remote server rejected the request")
}
