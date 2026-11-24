//! Serialized account lifecycle. System-store failures never become memory-only sign-in.
#[cfg(test)]
#[path = "lifecycle_tests.rs"]
mod tests;
use super::{
    account_for, configuration, missing, refresh_due, remote,
    stored_account::{StoredAccount, RECORD_KEY},
    LaunchAccount, RemoteAgentAccount,
};
use crate::{
    core::error::AppError,
    features::remote_agent::*,
    integration::{
        credentials::{CredentialError, CredentialStore},
        dsh_remote::provider::DshRemoteProvider,
    },
};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum RemoteCredentialPersistence {
    LaunchOnly,
    SystemStore,
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum RemoteCredentialState {
    NotChecked,
    LaunchOnly,
    Missing,
    Ready,
    Locked,
    Unavailable,
    Invalid,
    ReauthenticationRequired,
    ConnectionFailed,
}
pub struct RemoteAgentAccounts {
    pub(super) current: Mutex<AccountState>,
    pub(super) store: Option<CredentialStore>,
}
pub(super) struct AccountState {
    pub account: Option<LaunchAccount>,
    pub credential_state: RemoteCredentialState,
}
impl Default for RemoteAgentAccounts {
    fn default() -> Self {
        Self::with_store(None)
    }
}
pub(super) fn now_ms() -> Result<u64, AppError> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| missing())?
        .as_millis() as u64)
}
pub(super) fn credential_error(error: CredentialError) -> AppError {
    match error {
        CredentialError::Locked => AppError::domain(
            "remote_agent_credentials_locked",
            "Unlock the system credential store and try again",
        ),
        CredentialError::Unavailable => AppError::domain(
            "remote_agent_credentials_unavailable",
            "The system credential store is unavailable",
        ),
        CredentialError::InvalidRecord | CredentialError::InvalidKey => AppError::domain(
            "remote_agent_saved_credentials_invalid",
            "Saved Remote credentials are invalid; clear the saved sign-in",
        ),
    }
}
fn state_for(error: &AppError) -> RemoteCredentialState {
    match error.code() {
        "remote_agent_credentials_locked" => RemoteCredentialState::Locked,
        "remote_agent_credentials_unavailable" => RemoteCredentialState::Unavailable,
        "remote_agent_saved_credentials_invalid" => RemoteCredentialState::Invalid,
        "remote_agent_reauthentication_required" => RemoteCredentialState::ReauthenticationRequired,
        _ => RemoteCredentialState::ConnectionFailed,
    }
}
pub(super) async fn persist_account(
    account: &LaunchAccount,
    store: Option<&CredentialStore>,
) -> Result<(), AppError> {
    if let Some(store) = store {
        store
            .write(RECORD_KEY, &StoredAccount::capture(account)?)
            .await
            .map_err(credential_error)?;
    }
    Ok(())
}
async fn load_account(
    store: &CredentialStore,
    now: u64,
) -> Result<Option<LaunchAccount>, AppError> {
    let Some(saved) = store
        .read::<StoredAccount>(RECORD_KEY)
        .await
        .map_err(credential_error)?
    else {
        return Ok(None);
    };
    let restored = saved.restore(now)?;
    let expires_at = restored.credentials.access_token_expires_at;
    Ok(Some(LaunchAccount {
        api: restored.api,
        identity: restored.identity,
        credentials: restored.credentials,
        refresh_pending: false,
        pins: restored.pins,
        view: RemoteAgentAccount {
            account: restored.account,
            server: restored.server,
            expires_at,
            persistence: RemoteCredentialPersistence::SystemStore,
            login_provider: restored.login_provider,
            info: ConnectionInfo {
                connection: restored.connection,
                capabilities: Capabilities {
                    operations: vec![],
                    authentication: vec![],
                    backends: vec![],
                    attachments: None,
                },
            },
        },
    }))
}
async fn refresh_account(
    account: &mut LaunchAccount,
    provider: &DshRemoteProvider,
    store: Option<&CredentialStore>,
) -> Result<(), AppError> {
    let now = now_ms()?;
    if account.refresh_pending || account.credentials.refresh_token_expires_at <= now {
        return Err(AppError::domain(
            "remote_agent_reauthentication_required",
            "Clear the saved sign-in and sign in again",
        ));
    }
    if !refresh_due(account.credentials.access_token_expires_at, now) {
        return Ok(());
    }
    // Persist uncertainty before HTTP. Cancellation at any later await cannot authorize another refresh.
    account.refresh_pending = true;
    persist_account(account, store).await?;
    let refreshed = account
        .api
        .refresh(
            &account.identity.device_id,
            &account.credentials.refresh_token,
        )
        .await
        .map_err(|_| {
            AppError::domain(
                "remote_agent_reauthentication_required",
                "The refresh outcome is uncertain; clear the saved sign-in and sign in again",
            )
        })?;
    account.credentials = refreshed;
    account.view.expires_at = account.credentials.access_token_expires_at;
    // Save the rotated tokens while the durable uncertainty flag is still set.
    persist_account(account, store).await?;
    provider
        .update_access_token(
            &account.view.info.connection,
            account.credentials.access_token.clone(),
        )
        .await
        .map_err(remote)?;
    if let Some(store) = store {
        let certain = StoredAccount::capture(account)?.with_refresh_pending(false);
        store
            .write(RECORD_KEY, &certain)
            .await
            .map_err(credential_error)?;
    }
    // The live flag clears only after the final durable write succeeds.
    account.refresh_pending = false;
    Ok(())
}
impl RemoteAgentAccounts {
    pub fn with_store(store: Option<CredentialStore>) -> Self {
        let credential_state = if store.is_some() {
            RemoteCredentialState::NotChecked
        } else {
            RemoteCredentialState::LaunchOnly
        };
        Self {
            current: Mutex::new(AccountState {
                account: None,
                credential_state,
            }),
            store,
        }
    }
    pub(super) fn persistence(&self) -> RemoteCredentialPersistence {
        if self.store.is_some() {
            RemoteCredentialPersistence::SystemStore
        } else {
            RemoteCredentialPersistence::LaunchOnly
        }
    }
    pub(super) async fn ensure_restored(
        &self,
        service: &RemoteAgentService,
        provider: &DshRemoteProvider,
    ) {
        let Some(store) = self.store.as_ref() else {
            return;
        };
        let mut current = self.current.lock().await;
        if let Some(account) = current.account.as_ref() {
            if matches!(
                current.credential_state,
                RemoteCredentialState::Locked | RemoteCredentialState::Unavailable
            ) {
                // Retry only system storage. Never re-run refresh or Agent operations here.
                let requires_sign_in = account.refresh_pending
                    || now_ms().map_or(true, |now| {
                        account.credentials.refresh_token_expires_at <= now
                    });
                current.credential_state = match persist_account(account, Some(store)).await {
                    Ok(()) if requires_sign_in => RemoteCredentialState::ReauthenticationRequired,
                    Ok(()) => RemoteCredentialState::Ready,
                    Err(error) => state_for(&error),
                };
            }
            return;
        }
        if matches!(
            current.credential_state,
            RemoteCredentialState::Missing
                | RemoteCredentialState::Ready
                | RemoteCredentialState::Invalid
                | RemoteCredentialState::ReauthenticationRequired
        ) {
            return;
        }
        let loaded = match now_ms() {
            Ok(now) => load_account(store, now).await,
            Err(error) => Err(error),
        };
        match loaded {
            Ok(None) => current.credential_state = RemoteCredentialState::Missing,
            Ok(Some(account)) => {
                let _ = current
                    .activate(account, self.store.as_ref(), service, provider)
                    .await;
            }
            Err(error) => current.credential_state = state_for(&error),
        }
    }
}
impl AccountState {
    pub(super) fn can_sign_in(&self, persistence: RemoteCredentialPersistence) -> bool {
        self.account.is_none()
            && (persistence == RemoteCredentialPersistence::LaunchOnly
                || self.credential_state == RemoteCredentialState::Missing)
    }
    pub(super) async fn activate(
        &mut self,
        mut account: LaunchAccount,
        store: Option<&CredentialStore>,
        service: &RemoteAgentService,
        provider: &DshRemoteProvider,
    ) -> Result<RemoteAgentAccount, AppError> {
        let connection = account.view.info.connection.clone();
        self.credential_state = RemoteCredentialState::NotChecked;
        let result = async {
            persist_account(&account, store).await?;
            provider
                .configure(connection.connection_id.0.clone(), configuration(&account))
                .await
                .map_err(remote)?;
            refresh_account(&mut account, provider, store).await?;
            let info = service
                .connect(&ConnectRequest {
                    connection: connection.clone(),
                    config_version: 1,
                })
                .await?;
            account.view.info = info;
            account.view.expires_at = account.credentials.access_token_expires_at;
            Ok(account.view.clone())
        }
        .await;
        match result {
            Ok(view) => {
                self.account = Some(account);
                self.credential_state = RemoteCredentialState::Ready;
                Ok(view)
            }
            Err(error) => {
                let _ = provider.forget(&connection).await;
                self.credential_state = state_for(&error);
                Err(error)
            }
        }
    }
    pub(super) async fn refresh(
        &mut self,
        connection: &ConnectionRef,
        provider: &DshRemoteProvider,
        store: Option<&CredentialStore>,
    ) -> Result<(), AppError> {
        let result =
            refresh_account(account_for(&mut self.account, connection)?, provider, store).await;
        if let Err(error) = &result {
            self.credential_state = state_for(error);
        }
        result
    }
    pub(super) async fn pin(
        &mut self,
        connection: &ConnectionRef,
        host: String,
        key: String,
        store: Option<&CredentialStore>,
    ) -> Result<(), AppError> {
        let account = account_for(&mut self.account, connection)?;
        let mut pins = account.pins.clone();
        pins.insert(host, key);
        if let Some(store) = store {
            let saved = StoredAccount::capture(account)?.with_pins(pins.clone());
            if let Err(error) = store
                .write(RECORD_KEY, &saved)
                .await
                .map_err(credential_error)
            {
                self.credential_state = state_for(&error);
                return Err(error);
            }
        }
        account.pins = pins;
        self.credential_state = if account.refresh_pending {
            RemoteCredentialState::ReauthenticationRequired
        } else {
            RemoteCredentialState::Ready
        };
        Ok(())
    }
    pub(super) async fn clear(
        &mut self,
        store: Option<&CredentialStore>,
        service: &RemoteAgentService,
        provider: &DshRemoteProvider,
    ) -> Result<(), AppError> {
        // Removal runs even after locked/corrupt/expired restore and even when no account is loaded.
        if let Some(store) = store {
            if let Err(error) = store.remove(RECORD_KEY).await.map_err(credential_error) {
                self.credential_state = state_for(&error);
                return Err(error);
            }
        }
        self.credential_state = if store.is_some() {
            RemoteCredentialState::Missing
        } else {
            RemoteCredentialState::LaunchOnly
        };
        if let Some(account) = self.account.take() {
            let connection = account.view.info.connection;
            let disconnected = service.disconnect(&connection).await;
            provider.forget(&connection).await.map_err(remote)?;
            disconnected?;
        }
        Ok(())
    }
}
