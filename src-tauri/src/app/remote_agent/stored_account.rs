//! Private credential-store record. This type must never be exported through IPC.
use super::{remote, LaunchAccount};
use crate::{
    core::error::AppError,
    features::remote_agent::{ConnectionId, ConnectionRef, DEFAULT_PROVIDER_ID},
    integration::dsh_remote::{
        api::{validate_server_url, ServerApi, TokenPair},
        crypto::DeviceIdentity,
        protocol::key,
    },
};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, time::Duration};

pub(super) const RECORD_KEY: &str = "remote-agent.dsh.account.v1";

// No Debug or specta::Type. Only the system credential backend sees this value.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct StoredAccount {
    schema_version: u32,
    provider: String,
    connection_id: String,
    server: String,
    account: String,
    device_id: String,
    private_key: String,
    credentials: StoredTokens,
    refresh_pending: bool,
    pins: HashMap<String, String>,
    /// Third-party sign-in provider ID; absent in records saved before
    /// provider tracking existed (schema stays version 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    login_provider: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredTokens {
    access_token: String,
    access_token_expires_at: u64,
    refresh_token: String,
    refresh_token_expires_at: u64,
}

pub(super) struct RestoredAccount {
    pub api: ServerApi,
    pub identity: DeviceIdentity,
    pub credentials: TokenPair,
    pub pins: HashMap<String, String>,
    pub connection: ConnectionRef,
    pub server: String,
    pub account: String,
    pub login_provider: Option<String>,
}

fn invalid() -> AppError {
    AppError::domain(
        "remote_agent_saved_credentials_invalid",
        "Saved Remote credentials are invalid or require signing in again",
    )
}

impl StoredAccount {
    pub(super) fn with_refresh_pending(mut self, pending: bool) -> Self {
        self.refresh_pending = pending;
        self
    }
    pub(super) fn with_pins(mut self, pins: HashMap<String, String>) -> Self {
        self.pins = pins;
        self
    }
    pub(super) fn capture(account: &LaunchAccount) -> Result<Self, AppError> {
        Ok(Self {
            schema_version: 1,
            provider: DEFAULT_PROVIDER_ID.to_owned(),
            connection_id: account.view.info.connection.connection_id.0.clone(),
            server: validate_server_url(&account.view.server)
                .map_err(remote)?
                .to_string(),
            account: account.view.account.clone(),
            device_id: account.identity.device_id.clone(),
            private_key: account.identity.private_key_for_storage(),
            credentials: StoredTokens {
                access_token: account.credentials.access_token.clone(),
                access_token_expires_at: account.credentials.access_token_expires_at,
                refresh_token: account.credentials.refresh_token.clone(),
                refresh_token_expires_at: account.credentials.refresh_token_expires_at,
            },
            refresh_pending: account.refresh_pending,
            pins: account.pins.clone(),
            login_provider: account.view.login_provider.clone(),
        })
    }

    pub(super) fn restore(self, now_ms: u64) -> Result<RestoredAccount, AppError> {
        if self.schema_version != 1
            || self.provider != DEFAULT_PROVIDER_ID
            || uuid::Uuid::parse_str(&self.connection_id).is_err()
            || uuid::Uuid::parse_str(&self.device_id).is_err()
            || self.account.is_empty()
            || self.account.len() > 320
            || self.account.chars().any(char::is_control)
            || self.pins.len() > 512
            || self.login_provider.as_deref().is_some_and(|id| {
                id.is_empty()
                    || id.len() > 128
                    || !id.starts_with(|c: char| c.is_ascii_alphanumeric())
                    || !id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            })
        {
            return Err(invalid());
        }
        if self.refresh_pending || self.credentials.refresh_token_expires_at <= now_ms {
            return Err(AppError::domain(
                "remote_agent_reauthentication_required",
                "Saved Remote credentials require signing in again",
            ));
        }
        for (host, public_key) in &self.pins {
            if host.is_empty()
                || host.len() > 128
                || host
                    .chars()
                    .any(|c| c.is_control() || matches!(c, '/' | '\\'))
            {
                return Err(invalid());
            }
            key(public_key).map_err(|_| invalid())?;
        }
        let credentials = TokenPair {
            access_token: self.credentials.access_token,
            access_token_expires_at: self.credentials.access_token_expires_at,
            refresh_token: self.credentials.refresh_token,
            refresh_token_expires_at: self.credentials.refresh_token_expires_at,
        };
        credentials.validate().map_err(|_| invalid())?;
        let api = ServerApi::new(&self.server, Duration::from_secs(20)).map_err(remote)?;
        let identity =
            DeviceIdentity::restore(self.device_id, &self.private_key).map_err(remote)?;
        Ok(RestoredAccount {
            api,
            identity,
            credentials,
            pins: self.pins,
            connection: ConnectionRef {
                provider_id: DEFAULT_PROVIDER_ID.into(),
                connection_id: ConnectionId(self.connection_id),
            },
            server: self.server,
            account: self.account,
            login_provider: self.login_provider,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integration::dsh_remote::protocol::encode_base64;
    use serde_json::{json, Value};

    fn record() -> Value {
        json!({
            "schemaVersion": 1,
            "provider": DEFAULT_PROVIDER_ID,
            "connectionId": uuid::Uuid::new_v4().to_string(),
            "server": "https://remote.example/",
            "account": "reader@example.test",
            "deviceId": uuid::Uuid::new_v4().to_string(),
            "privateKey": encode_base64(&[7;32]),
            "credentials": {
                "accessToken": "access-token-fixture",
                "accessTokenExpiresAt": 100,
                "refreshToken": "refresh-token-fixture",
                "refreshTokenExpiresAt": 1000
            },
            "refreshPending": false,
            "pins": { "host-1": encode_base64(&[9;32]) }
        })
    }

    #[tokio::test]
    async fn restore_keeps_identity_connection_and_pins_with_expired_access_token() {
        let value = record();
        let saved: StoredAccount = serde_json::from_value(value.clone()).unwrap();
        let restored = saved.restore(200).unwrap();
        assert_eq!(restored.connection.connection_id.0, value["connectionId"]);
        assert_eq!(restored.identity.device_id, value["deviceId"]);
        assert_eq!(
            restored.identity.private_key_for_storage(),
            value["privateKey"]
        );
        assert_eq!(
            restored.pins.get("host-1").unwrap(),
            value["pins"]["host-1"].as_str().unwrap()
        );
        assert_eq!(restored.credentials.access_token_expires_at, 100);
        // Records saved before provider tracking existed restore without one.
        assert_eq!(restored.login_provider, None);
    }

    #[tokio::test]
    async fn login_provider_round_trips_and_must_be_a_plain_provider_id() {
        let mut value = record();
        value["loginProvider"] = json!("github");
        let restored = serde_json::from_value::<StoredAccount>(value)
            .unwrap()
            .restore(200)
            .unwrap();
        assert_eq!(restored.login_provider.as_deref(), Some("github"));
        for bad in [json!(""), json!("../evil"), json!("has space")] {
            let mut value = record();
            value["loginProvider"] = bad.clone();
            assert!(
                serde_json::from_value::<StoredAccount>(value)
                    .unwrap()
                    .restore(200)
                    .is_err(),
                "{bad}"
            );
        }
        // Non-string provider values fail closed at deserialization.
        let mut value = record();
        value["loginProvider"] = json!(42);
        assert!(serde_json::from_value::<StoredAccount>(value).is_err());
    }

    #[tokio::test]
    async fn uncertain_refresh_and_expired_refresh_cannot_resume() {
        for (pending, expiry) in [(true, 1000), (false, 200)] {
            let mut value = record();
            value["refreshPending"] = json!(pending);
            value["credentials"]["refreshTokenExpiresAt"] = json!(expiry);
            assert!(serde_json::from_value::<StoredAccount>(value)
                .unwrap()
                .restore(200)
                .is_err());
        }
    }

    #[tokio::test]
    async fn invalid_versions_namespaces_origins_keys_and_identity_are_rejected() {
        let edits = [
            ("schemaVersion", json!(2)),
            ("provider", json!("another-provider")),
            ("server", json!("http://public.example/")),
            ("connectionId", json!("invalid")),
            ("deviceId", json!("invalid")),
            ("privateKey", json!("invalid")),
            ("pins", json!({"../host": encode_base64(&[9;32])})),
        ];
        for (field, edit) in edits {
            let mut value = record();
            value[field] = edit;
            assert!(
                serde_json::from_value::<StoredAccount>(value)
                    .unwrap()
                    .restore(200)
                    .is_err(),
                "{field}"
            );
        }
        let mut value = record();
        value["password"] = json!("must-not-be-stored");
        assert!(serde_json::from_value::<StoredAccount>(value).is_err());
    }
}
