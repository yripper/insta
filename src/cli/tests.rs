use super::daemon::ensure_bearer_token;
use super::probe::shape;
use super::*;
use crate::ig::InstagramApi;
use crate::ig::fake::{FakeData, FakeInstagram, me};
use crate::ig::types::{IgError, NOT_IMPLEMENTED};
use crate::secrets::tests_support::MemSecrets;
use async_trait::async_trait;
use std::sync::Arc;

struct FakeConnector(Result<Me, IgError>);

#[async_trait]
impl Connector for FakeConnector {
    async fn connect(&self, _c: Credentials) -> std::result::Result<(Arc<dyn InstagramApi>, Me), IgError> {
        let me = self.0.clone()?;
        Ok((Arc::new(FakeInstagram::new(FakeData::default())), me))
    }
}

fn creds() -> Credentials {
    Credentials { sessionid: "001%3Aabc%3A12".into(), csrftoken: "C1".into(), ds_user_id: "U001".into() }
}

#[tokio::test]
async fn bearer_token_created_once() {
    let s = MemSecrets::default();
    let a = ensure_bearer_token(&s).await.unwrap();
    let b = ensure_bearer_token(&s).await.unwrap();
    assert_eq!(a, b);
    assert_eq!(a.len(), 64);
    assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
}

#[tokio::test]
async fn login_validates_saves_and_clears_challenge() {
    let s = MemSecrets::default();
    let store = Store::open_in_memory().unwrap();
    store.set_challenge(1).unwrap();
    let out = save_login(&FakeConnector(Ok(me())), &s, &store, creds()).await.unwrap();
    assert_eq!(out, LoginOutcome::Validated(me()));
    assert_eq!(load_credentials(&s).await.unwrap(), Some(creds()));
    assert_eq!(store.challenge_at().unwrap(), None);
}

#[tokio::test]
async fn login_saves_with_validation_pending_while_client_is_a_stub() {
    let s = MemSecrets::default();
    let store = Store::open_in_memory().unwrap();
    let c = FakeConnector(Err(IgError::Http(NOT_IMPLEMENTED.into())));
    assert_eq!(save_login(&c, &s, &store, creds()).await.unwrap(), LoginOutcome::ValidationPending);
    assert!(load_credentials(&s).await.unwrap().is_some());
}

#[tokio::test]
async fn login_rejects_bad_session_and_bad_format_without_saving() {
    let s = MemSecrets::default();
    let store = Store::open_in_memory().unwrap();
    assert!(save_login(&FakeConnector(Err(IgError::NeedsLogin)), &s, &store, creds()).await.is_err());
    let quoted = Credentials { csrftoken: "\"C1\"".into(), ..creds() };
    let e = save_login(&FakeConnector(Ok(me())), &s, &store, quoted).await.unwrap_err().to_string();
    assert!(e.contains("csrftoken"), "{e}");
    assert!(!e.contains("C1"), "never echo the value");
    let empty = Credentials { ds_user_id: String::new(), ..creds() };
    assert!(save_login(&FakeConnector(Ok(me())), &s, &store, empty).await.is_err());
    assert_eq!(load_credentials(&s).await.unwrap(), None);
}

#[test]
fn shape_prints_keys_and_types_not_values() {
    let v = serde_json::json!({
        "status": "ok",
        "inbox": {"threads": [{"thread_id": "TH001", "users": [{"pk": 1234567, "username": "secret_user"}], "muted": false}]},
        "by_id": {"1234567890": {"seen": 3}},
        "1234567": "digits key",
        "empty": [],
        "nothing": null
    });
    let out = shape(&v);
    assert!(out.contains("threads: array[1] of object"), "{out}");
    assert!(out.contains("muted: bool"), "{out}");
    assert!(out.contains("by_id: map[1] of object"), "{out}");
    assert!(out.contains("<id>: string"), "{out}");
    for secret in ["TH001", "secret_user", "1234567", "digits key"] {
        assert!(!out.contains(secret), "leaked {secret}: {out}");
    }
}
