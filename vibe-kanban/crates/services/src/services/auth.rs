use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use api_types::ProfileResponse;
use tokio::sync::{Mutex as TokioMutex, OwnedMutexGuard, RwLock};

use super::oauth_credentials::{Credentials, OAuthCredentials};

const DEFAULT_AUTH_SESSION_ID: &str = "__default__";
const VIBE_KANBAN_BROWSER_SCOPED_AUTH_ENV: &str = "VIBE_KANBAN_BROWSER_SCOPED_AUTH";

tokio::task_local! {
    static AUTH_SESSION_ID: String;
}

#[derive(Clone)]
struct SessionState {
    oauth: Arc<OAuthCredentials>,
    profile: Arc<RwLock<Option<ProfileResponse>>>,
    remote_auth_degraded_slug: Arc<RwLock<Option<String>>>,
    refresh_lock: Arc<TokioMutex<()>>,
}

#[derive(Clone)]
pub struct AuthContext {
    base_credentials_path: Arc<PathBuf>,
    browser_scoped_auth: bool,
    session_states: Arc<RwLock<HashMap<String, SessionState>>>,
}

impl AuthContext {
    pub fn new(base_credentials_path: PathBuf) -> Self {
        Self {
            base_credentials_path: Arc::new(base_credentials_path),
            browser_scoped_auth: parse_bool_env(VIBE_KANBAN_BROWSER_SCOPED_AUTH_ENV).unwrap_or(true),
            session_states: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn run_with_session<F, T>(&self, session_id: String, future: F) -> T
    where
        F: std::future::Future<Output = T>,
    {
        AUTH_SESSION_ID.scope(session_id, future).await
    }

    pub fn current_session_id(&self) -> Option<String> {
        AUTH_SESSION_ID.try_with(Clone::clone).ok()
    }

    pub async fn get_credentials(&self) -> Option<Credentials> {
        let current_session_id = self.current_session_id();
        self.get_credentials_for(current_session_id.as_deref()).await
    }

    pub async fn get_credentials_for(&self, session_id: Option<&str>) -> Option<Credentials> {
        self.session_state_for(session_id).await.oauth.get().await
    }

    pub async fn save_credentials(&self, creds: &Credentials) -> std::io::Result<()> {
        let current_session_id = self.current_session_id();
        self.save_credentials_for(current_session_id.as_deref(), creds)
            .await
    }

    pub async fn save_credentials_for(
        &self,
        session_id: Option<&str>,
        creds: &Credentials,
    ) -> std::io::Result<()> {
        self.session_state_for(session_id).await.oauth.save(creds).await
    }

    pub async fn clear_credentials(&self) -> std::io::Result<()> {
        let current_session_id = self.current_session_id();
        self.clear_credentials_for(current_session_id.as_deref()).await
    }

    pub async fn clear_credentials_for(&self, session_id: Option<&str>) -> std::io::Result<()> {
        self.session_state_for(session_id).await.oauth.clear().await
    }

    pub async fn remote_auth_degraded_slug(&self) -> Option<String> {
        let current_session_id = self.current_session_id();
        self.session_state_for(current_session_id.as_deref())
            .await
            .remote_auth_degraded_slug
            .read()
            .await
            .clone()
    }

    pub async fn set_remote_auth_degraded_slug(&self, slug: impl Into<String>) {
        let current_session_id = self.current_session_id();
        *self
            .session_state_for(current_session_id.as_deref())
            .await
            .remote_auth_degraded_slug
            .write()
            .await = Some(slug.into());
    }

    pub async fn clear_remote_auth_degraded_slug(&self) {
        let current_session_id = self.current_session_id();
        *self
            .session_state_for(current_session_id.as_deref())
            .await
            .remote_auth_degraded_slug
            .write()
            .await = None;
    }

    pub async fn cached_profile(&self) -> Option<ProfileResponse> {
        let current_session_id = self.current_session_id();
        self.cached_profile_for(current_session_id.as_deref()).await
    }

    pub async fn cached_profile_for(&self, session_id: Option<&str>) -> Option<ProfileResponse> {
        self.session_state_for(session_id).await.profile.read().await.clone()
    }

    pub async fn set_profile(&self, profile: ProfileResponse) {
        let current_session_id = self.current_session_id();
        self.set_profile_for(current_session_id.as_deref(), profile).await;
    }

    pub async fn set_profile_for(&self, session_id: Option<&str>, profile: ProfileResponse) {
        *self.session_state_for(session_id).await.profile.write().await = Some(profile);
    }

    pub async fn clear_profile(&self) {
        let current_session_id = self.current_session_id();
        self.clear_profile_for(current_session_id.as_deref()).await;
    }

    pub async fn clear_profile_for(&self, session_id: Option<&str>) {
        *self.session_state_for(session_id).await.profile.write().await = None;
    }

    pub async fn refresh_guard(&self) -> OwnedMutexGuard<()> {
        let current_session_id = self.current_session_id();
        self.refresh_guard_for(current_session_id.as_deref()).await
    }

    pub async fn refresh_guard_for(&self, session_id: Option<&str>) -> OwnedMutexGuard<()> {
        self.session_state_for(session_id)
            .await
            .refresh_lock
            .clone()
            .lock_owned()
            .await
    }

    fn normalize_session_id(&self, session_id: Option<&str>) -> String {
        if !self.browser_scoped_auth {
            return DEFAULT_AUTH_SESSION_ID.to_string();
        }

        session_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(DEFAULT_AUTH_SESSION_ID)
            .to_string()
    }

    fn credentials_path_for_session(&self, session_id: &str) -> PathBuf {
        if session_id == DEFAULT_AUTH_SESSION_ID {
            return self.base_credentials_path.as_ref().clone();
        }

        let parent_dir = self
            .base_credentials_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        parent_dir
            .join("credentials")
            .join(format!("{session_id}.json"))
    }

    async fn session_state_for(&self, session_id: Option<&str>) -> SessionState {
        let session_id = self.normalize_session_id(session_id);

        if let Some(state) = self.session_states.read().await.get(&session_id).cloned() {
            return state;
        }

        let oauth = Arc::new(OAuthCredentials::new(
            self.credentials_path_for_session(&session_id),
        ));
        if let Err(error) = oauth.load().await {
            tracing::warn!(?error, session_id = %session_id, "failed to load OAuth credentials");
        }

        let state = SessionState {
            oauth,
            profile: Arc::new(RwLock::new(None)),
            remote_auth_degraded_slug: Arc::new(RwLock::new(None)),
            refresh_lock: Arc::new(TokioMutex::new(())),
        };

        let mut session_states = self.session_states.write().await;
        if let Some(existing_state) = session_states.get(&session_id).cloned() {
            return existing_state;
        }
        session_states.insert(session_id, state.clone());
        state
    }
}

fn parse_bool_env(name: &str) -> Option<bool> {
    let value = std::env::var(name).ok()?;

    match value.trim().to_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}
