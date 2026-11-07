use super::*;
use crate::ig::fake::{FakeData, FakeInstagram, dm, group, user};

fn c(id: &str, username: &str, full: &str) -> CachedThread {
    CachedThread {
        thread_id: id.into(),
        peer_id: format!("P{id}"),
        username: username.into(),
        full_name: full.into(),
        fetched_at: 0,
    }
}

fn cache() -> Vec<CachedThread> {
    vec![c("TH001", "ana", "Ana Example"), c("TH002", "anabel", "Bel Example"), c("TH003", "bo_x", "")]
}

fn ids(r: Resolved) -> Vec<String> {
    match r {
        Resolved::One(t) => vec![t.thread_id],
        Resolved::Many(ts) => ts.into_iter().map(|t| t.thread_id).collect(),
        Resolved::None => vec![],
    }
}

#[test]
fn resolves_by_id_username_and_name() {
    assert_eq!(ids(resolve("TH003", &cache())), ["TH003"]);
    assert_eq!(ids(resolve("@ana", &cache())), ["TH001"], "exact username beats prefix");
    assert_eq!(ids(resolve("ANA", &cache())), ["TH001"]);
    assert_eq!(ids(resolve("bel example", &cache())), ["TH002"]);
    assert_eq!(ids(resolve("bo", &cache())), ["TH003"]);
}

#[test]
fn several_partial_matches_are_many_never_a_guess() {
    assert!(matches!(resolve("example", &cache()), Resolved::Many(v) if v.len() == 2));
    assert!(matches!(resolve("an", &cache()), Resolved::Many(v) if v.len() == 2));
}

#[test]
fn at_prefix_searches_usernames_only() {
    assert_eq!(resolve("@example", &cache()), Resolved::None);
}

#[test]
fn empty_and_unknown_queries_are_none() {
    assert_eq!(resolve("", &cache()), Resolved::None);
    assert_eq!(resolve("  @ ", &cache()), Resolved::None);
    assert_eq!(resolve("zed", &cache()), Resolved::None);
    assert_eq!(resolve("ana", &[]), Resolved::None);
}

#[test]
fn groups_are_not_cached_and_names_display() {
    assert_eq!(to_cached(&group("TH009", 1), 5), None);
    let t = to_cached(&dm("TH001", user("U001", "ana", "Ana Example"), 1, false), 5).unwrap();
    assert_eq!(t.fetched_at, 5);
    assert_eq!(display_name(&t), "@ana (Ana Example)");
    assert_eq!(display_name(&c("TH003", "bo_x", "")), "@bo_x");
}

#[tokio::test]
async fn refresh_pages_inbox_and_keeps_only_one_to_one() {
    let mut threads: Vec<_> = (0..25)
        .map(|i| dm(&format!("TH{i:03}"), user(&format!("U{i:03}"), &format!("u{i}"), ""), 100 - i, false))
        .collect();
    threads.insert(1, group("TH900", 99));
    let api = Arc::new(FakeInstagram::new(FakeData { threads, ..Default::default() }));
    let store = Arc::new(Store::open_in_memory().unwrap());
    let dir = Directory::new(api.clone(), store.clone());
    let got = dir.refresh(1_000).await.unwrap();
    assert_eq!(got.len(), 25);
    assert_eq!(api.count("inbox"), 2);
    assert_eq!(store.cached_threads().unwrap().len(), 25);
}

#[tokio::test]
async fn resolve_uses_cache_within_ttl_and_refreshes_once_on_miss() {
    let api = Arc::new(FakeInstagram::new(FakeData {
        threads: vec![dm("TH001", user("U001", "ana", ""), 10, false)],
        ..Default::default()
    }));
    let store = Arc::new(Store::open_in_memory().unwrap());
    let dir = Directory::new(api.clone(), store);
    assert_eq!(dir.resolve_one("ana", 1_000).await.unwrap().thread_id, "TH001");
    assert_eq!(dir.resolve_one("@ana", 1_010).await.unwrap().thread_id, "TH001");
    assert_eq!(api.count("inbox"), 1, "second lookup is served from cache");

    api.data.lock().unwrap().threads.push(dm("TH002", user("U002", "newbie", ""), 20, false));
    // Miss within a minute of the last refresh: no extra call.
    assert!(matches!(dir.resolve_one("newbie", 1_030).await, Err(Error::NotFound(_))));
    assert_eq!(api.count("inbox"), 1);
    // Miss later: one refresh finds it.
    assert_eq!(dir.resolve_one("newbie", 1_100).await.unwrap().thread_id, "TH002");
    assert_eq!(api.count("inbox"), 2);
}

#[tokio::test]
async fn ambiguous_error_lists_candidates_with_ids() {
    let api = Arc::new(FakeInstagram::new(FakeData {
        threads: vec![
            dm("TH001", user("U001", "ana", ""), 10, false),
            dm("TH002", user("U002", "anabel", ""), 9, false),
        ],
        ..Default::default()
    }));
    let dir = Directory::new(api, Arc::new(Store::open_in_memory().unwrap()));
    let e = dir.resolve_one("an", 1_000).await.unwrap_err().to_string();
    assert!(e.contains("@ana (TH001)") && e.contains("@anabel (TH002)"), "{e}");
}
