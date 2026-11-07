use super::*;
use crate::ig::fake::{FakeData, FakeInstagram, ME_ID, text, voice};
use crate::store::CachedThread;

const NOW: i64 = 1_800_000_000;
const DAY: i64 = 86_400;

fn us(secs: i64) -> i64 {
    secs * 1_000_000
}

#[test]
fn staleness_by_count_or_age() {
    assert!(!is_stale(NOW, 29, NOW));
    assert!(is_stale(NOW, 30, NOW));
    assert!(!is_stale(NOW - STALE_AGE_SECS + 1, 0, NOW));
    assert!(is_stale(NOW - STALE_AGE_SECS, 0, NOW));
}

#[test]
fn chat_with_no_notes_needs_notes_and_no_profile_needs_learning() {
    let s = Store::open_in_memory().unwrap();
    let ctx = voice_context(&s, "TH001", NOW).unwrap();
    assert_eq!(
        ctx,
        VoiceContext {
            category: "general".into(),
            profile: None,
            notes: None,
            profile_stale: false,
            notes_stale: true,
            needs_learning: true,
        }
    );
}

#[test]
fn fresh_profile_and_notes_need_nothing_until_30_own_messages() {
    let s = Store::open_in_memory().unwrap();
    s.set_category("TH001", "friends").unwrap();
    s.save_profile("friends", "casual", NOW - DAY).unwrap();
    s.save_notes("TH001", "jaja a lot", NOW - DAY).unwrap();
    let ctx = voice_context(&s, "TH001", NOW).unwrap();
    assert!(!ctx.needs_learning && !ctx.notes_stale && !ctx.profile_stale);
    assert_eq!(ctx.profile.as_deref(), Some("casual"));

    let items: Vec<(String, i64)> = (0..30).map(|i| (format!("I{i:03}"), us(NOW - DAY + 1 + i))).collect();
    s.record_own_activity("TH001", &items, NOW).unwrap();
    let ctx = voice_context(&s, "TH001", NOW).unwrap();
    assert!(ctx.notes_stale && ctx.profile_stale && ctx.needs_learning);
    assert_eq!(stale_profiles(&s, NOW).unwrap(), ["friends"]);
}

#[test]
fn messages_insta_sent_do_not_count_toward_staleness() {
    let s = Store::open_in_memory().unwrap();
    s.save_notes("TH001", "n", NOW - DAY).unwrap();
    let items: Vec<(String, i64)> = (0..30).map(|i| (format!("I{i:03}"), us(NOW - DAY + 1 + i))).collect();
    s.record_own_activity("TH001", &items, NOW).unwrap();
    s.record_sent("I000", "TH001", us(NOW - DAY + 1)).unwrap();
    assert!(!voice_context(&s, "TH001", NOW).unwrap().notes_stale);
}

#[test]
fn old_profile_is_stale_by_age() {
    let s = Store::open_in_memory().unwrap();
    s.save_profile("general", "x", NOW - STALE_AGE_SECS).unwrap();
    s.save_profile("work", "y", NOW).unwrap();
    assert_eq!(stale_profiles(&s, NOW).unwrap(), ["general"]);
}

#[test]
fn category_slug_rules() {
    for ok in ["general", "work-2", "a_b", "x"] {
        assert!(valid_category(ok), "{ok}");
    }
    for bad in ["", "Work", "with space", "ñ", &"a".repeat(33)] {
        assert!(!valid_category(bad), "{bad}");
    }
}

#[test]
fn own_texts_skip_sent_media_incoming_and_blank() {
    let msgs = vec![
        text("I001", 5, ME_ID, "mine"),
        text("I002", 4, ME_ID, "sent by insta"),
        text("I003", 3, "U001", "theirs"),
        voice("I004", 2, ME_ID, "https://cdn.example.com/v", 1_000),
        text("I005", 1, ME_ID, "   "),
    ];
    let sent: HashSet<String> = ["I002".to_string()].into();
    let got: Vec<&str> = own_texts(&msgs, &sent).iter().map(|m| m.id.as_str()).collect();
    assert_eq!(got, ["I001"]);
}

fn api_with(threads: &[(&str, usize)]) -> FakeInstagram {
    let mut d = FakeData::default();
    for (id, n) in threads {
        let msgs = (0..*n).map(|i| text(&format!("{id}-{i}"), us(NOW) - i as i64, ME_ID, "fake text")).collect();
        d.messages.insert(id.to_string(), msgs);
    }
    FakeInstagram::new(d)
}

#[tokio::test]
async fn own_messages_pages_live_caps_and_records_activity() {
    let api = api_with(&[("TH001", 120)]);
    let s = Store::open_in_memory().unwrap();
    s.record_sent("TH001-0", "TH001", us(NOW)).unwrap();
    let got = own_messages(&api, &s, "TH001", CHAT_SAMPLE, NOW).await.unwrap();
    assert_eq!(got.len(), 60);
    assert_eq!(got[0].id, "TH001-1", "sent item excluded");
    assert_eq!(api.count("thread TH001"), 2);
    assert_eq!(s.own_count_in_thread("TH001", NOW - DAY).unwrap(), 99, "100 seen, 1 sent by insta");
}

#[tokio::test]
async fn category_sample_reads_at_most_three_other_threads() {
    let api = api_with(&[("TH001", 5), ("TH002", 30), ("TH003", 30), ("TH004", 30), ("TH005", 30)]);
    let s = Store::open_in_memory().unwrap();
    let rows: Vec<CachedThread> = ["TH001", "TH002", "TH003", "TH004", "TH005"]
        .iter()
        .enumerate()
        .map(|(i, id)| CachedThread {
            thread_id: id.to_string(),
            peer_id: format!("U{i}"),
            username: format!("u{i}"),
            full_name: String::new(),
            fetched_at: 100 - i as i64,
        })
        .collect();
    s.put_threads(&rows).unwrap();
    let got = own_messages_in_category(&api, &s, "general", "TH001", CATEGORY_SAMPLE, NOW).await.unwrap();
    assert_eq!(got.len(), 40);
    assert_eq!(api.count("thread TH001"), 0, "the chat itself is not part of the category sample");
    assert_eq!(api.count("thread TH002") + api.count("thread TH003"), 2);
    assert_eq!(api.count("thread TH004") + api.count("thread TH005"), 0, "stops once it has 40");
}
