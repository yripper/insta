//! Session state shared by every MCP request. Starts (or falls back to)
//! `NeedsLogin`, and reconnects from the keyring on the next call, so
//! `insta login` works without restarting the daemon. After a challenge it
//! refuses to touch Instagram at all until `insta login` clears the flag.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::domain::actions::Actions;
use crate::domain::clock::Timer;
use crate::domain::errors::{Error, Result};
use crate::domain::send_queue::SendQueue;
use crate::ig::types::{Credentials, IgError, Me};
use crate::ig::{HttpInstagram, InstagramApi};
use crate::secrets::{ACCOUNT_CSRFTOKEN, ACCOUNT_DS_USER_ID, ACCOUNT_SESSIONID, SecretStore};
use crate::store::Store;
use crate::whisper::Transcriber;

/// Builds an Instagram client from credentials and checks it with `whoami`.
#[async_trait]
pub trait Connector: Send + Sync {
    async fn connect(&self, creds: Credentials) -> std::result::Result<(Arc<dyn InstagramApi>, Me), IgError>;
}

pub struct HttpConnector;

#[async_trait]
impl Connector for HttpConnector {
    async fn connect(&self, creds: Credentials) -> std::result::Result<(Arc<dyn InstagramApi>, Me), IgError> {
        let api = HttpInstagram::new(creds)?;
        let me = api.whoami().await?;
        Ok((Arc::new(api), me))
    }
}

/// The three session cookies from the keyring, if all are there.
pub async fn load_credentials(secrets: &dyn SecretStore) -> anyhow::Result<Option<Credentials>> {
    let sessionid = secrets.get(ACCOUNT_SESSIONID).await?;
    let csrftoken = secrets.get(ACCOUNT_CSRFTOKEN).await?;
    let ds_user_id = secrets.get(ACCOUNT_DS_USER_ID).await?;
    Ok(match (sessionid, csrftoken, ds_user_id) {
        (Some(sessionid), Some(csrftoken), Some(ds_user_id)) => Some(Credentials { sessionid, csrftoken, ds_user_id }),
        _ => None,
    })
}

enum Session {
    Ready(Arc<Actions>),
    NeedsLogin,
}

pub struct AppState {
    store: Arc<Store>,
    secrets: Arc<dyn SecretStore>,
    connector: Arc<dyn Connector>,
    queue: Arc<SendQueue>,
    whisper: Arc<dyn Transcriber>,
    timer: Arc<dyn Timer>,
    session: RwLock<Session>,
}

pub struct Deps {
    pub store: Arc<Store>,
    pub secrets: Arc<dyn SecretStore>,
    pub connector: Arc<dyn Connector>,
    pub queue: Arc<SendQueue>,
    pub whisper: Arc<dyn Transcriber>,
    pub timer: Arc<dyn Timer>,
}

impl AppState {
    pub fn new(d: Deps) -> AppState {
        AppState {
            store: d.store,
            secrets: d.secrets,
            connector: d.connector,
            queue: d.queue,
            whisper: d.whisper,
            timer: d.timer,
            session: RwLock::new(Session::NeedsLogin),
        }
    }

    pub fn queue(&self) -> &Arc<SendQueue> {
        &self.queue
    }

    /// The ready session, reconnecting from the keyring if needed.
    pub async fn actions(&self) -> Result<Arc<Actions>> {
        if let Session::Ready(a) = &*self.session.read().await {
            return Ok(a.clone());
        }
        let mut guard = self.session.write().await;
        if let Session::Ready(a) = &*guard {
            return Ok(a.clone());
        }
        if self.store.challenge_at()?.is_some() {
            return Err(IgError::Challenge.into());
        }
        let Some(creds) = load_credentials(self.secrets.as_ref()).await? else {
            return Err(IgError::NeedsLogin.into());
        };
        let (api, me) = self.connector.connect(creds).await?;
        self.queue.attach(api.clone());
        let actions = Arc::new(Actions::new(
            api,
            self.store.clone(),
            me,
            self.queue.clone(),
            self.whisper.clone(),
            self.timer.clone(),
        ));
        *guard = Session::Ready(actions.clone());
        tracing::info!("instagram session ready");
        Ok(actions)
    }

    /// Called when a tool failed: a dead or challenged session is dropped, and
    /// a challenge is remembered across restarts.
    pub async fn note_error(&self, err: &Error) {
        if !err.is_fatal() {
            return;
        }
        *self.session.write().await = Session::NeedsLogin;
        if matches!(err, Error::Ig(IgError::Challenge)) {
            if let Err(e) = self.store.set_challenge(self.timer.now_secs()) {
                tracing::warn!("saving the challenge flag failed: {e}");
            }
            tracing::warn!("instagram challenge: all calls stopped until `insta login`");
        } else {
            tracing::warn!("instagram session expired; waiting for `insta login`");
        }
    }
}
