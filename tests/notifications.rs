use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::TcpListener,
    sync::{Arc, Mutex},
    thread,
};

use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::{TimeZone, Utc};
use chrono_tz::America::Los_Angeles;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use yearn_vote_notifications::{
    http::Sources,
    model::{EPOCH_SECONDS, Feed, GENESIS, Topic, VOTING, Vote, discussion_identity},
    poll,
    state::State,
};

fn vote(open: bool) -> Vote {
    Vote {
        id: "1:voting:1".into(),
        title: "Extend reward claims".into(),
        url: "https://dao.yearn.fi/proposals/1".into(),
        starts: 100,
        ends: i64::MAX,
        open,
        eligible: true,
    }
}
fn topic(id: u64) -> Topic {
    Topic {
        id,
        title: format!("Discussion {id}"),
        slug: "discussion".into(),
        created_at: Utc::now(),
        visible: true,
        archetype: "regular".into(),
    }
}

#[tokio::test]
async fn publication_then_opening_survives_restarts() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("state.sqlite");
    let state = State::load(path.clone(), false).await.unwrap();
    State::save(path.clone(), state.votes(vec![vote(false)], 100), 100)
        .await
        .unwrap();
    let pending = State::pending(path.clone(), "dao".into()).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert!(pending[0].key.ends_with(":published"));
    State::ack(path.clone(), pending[0].key.clone(), "receipt-1".into())
        .await
        .unwrap();
    let state = State::load(path.clone(), false).await.unwrap();
    State::save(path.clone(), state.votes(vec![vote(true)], 200), 200)
        .await
        .unwrap();
    let pending = State::pending(path.clone(), "dao".into()).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert!(pending[0].key.ends_with(":opened"));
    State::ack(path.clone(), pending[0].key.clone(), "receipt-2".into())
        .await
        .unwrap();
    let state = State::load(path.clone(), false).await.unwrap();
    State::save(path.clone(), state.votes(vec![vote(true)], 300), 300)
        .await
        .unwrap();
    assert!(State::pending(path, "dao".into()).await.unwrap().is_empty());
}

#[tokio::test]
async fn active_on_first_start_has_one_combined_alert() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("state.sqlite");
    let state = State::load(path.clone(), false).await.unwrap();
    State::save(path.clone(), state.votes(vec![vote(true)], 100), 100)
        .await
        .unwrap();
    let pending = State::pending(path, "dao".into()).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert!(pending[0].title.contains("voting open"));
}

#[tokio::test]
async fn publication_queued_during_outage_is_combined_when_vote_opens() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("state.sqlite");
    let state = State::load(path.clone(), false).await.unwrap();
    State::save(path.clone(), state.votes(vec![vote(false)], 100), 100)
        .await
        .unwrap();
    let state = State::load(path.clone(), false).await.unwrap();
    State::save(path.clone(), state.votes(vec![vote(true)], 200), 200)
        .await
        .unwrap();
    let pending = State::pending(path, "dao".into()).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert!(pending[0].key.ends_with(":opened"));
}

#[tokio::test]
async fn withdrawn_and_ended_votes_cancel_queued_delivery() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("state.sqlite");
    let state = State::load(path.clone(), false).await.unwrap();
    State::save(path.clone(), state.votes(vec![vote(false)], 100), 100)
        .await
        .unwrap();
    let state = State::load(path.clone(), false).await.unwrap();
    let mut withdrawn = vote(false);
    withdrawn.eligible = false;
    State::save(path.clone(), state.votes(vec![withdrawn], 200), 200)
        .await
        .unwrap();
    assert!(State::pending(path, "dao".into()).await.unwrap().is_empty());
}

#[tokio::test]
async fn discussion_baseline_ignores_history_and_edits() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("state.sqlite");
    let state = State::load(path.clone(), false).await.unwrap();
    State::save(path.clone(), state.discussions(vec![topic(1)], 100), 100)
        .await
        .unwrap();
    assert!(
        State::pending(path.clone(), "forum".into())
            .await
            .unwrap()
            .is_empty()
    );
    let state = State::load(path.clone(), false).await.unwrap();
    let mut edited = topic(1);
    edited.title = "Edited title".into();
    State::save(
        path.clone(),
        state.discussions(vec![edited, topic(2)], 200),
        200,
    )
    .await
    .unwrap();
    let pending = State::pending(path, "forum".into()).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].entity, "2");
}

fn fixture(now: i64) -> String {
    let content = r##"{"schema":"yearn.dao.proposal.v1","markdown":"# Summary","discussionUrl":"https://gov.yearn.fi/t/reward-claims/7"}"##;
    let bytes = STANDARD.encode(content);
    let hash = Sha256::digest(content.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let epoch = (now - GENESIS) / EPOCH_SECONDS;
    r#"{"schema":"yearn.dao.feed.v2","chainId":1,"observedAt":@NOW@,"block":{"number":"26354210","hash":"0xabc","timestamp":@NOW@},"deployments":[{"votingAddress":"@VOTING@","proposalCount":"1","configuration":{"voteStart":"0"}}],"proposals":[{"votingAddress":"@VOTING@","id":"0","epoch":"@EPOCH@","contentDigest":"0x@HASH@","contentBytes":"@BYTES@","status":"VOTING","retracted":false,"executed":false,"vetoed":false,"flagged":false,"events":[{"type":"propose","log":{"blockNumber":"26354200","blockHash":"0xabc","timestamp":@NOW@}}]}]}"#
        .replace("@NOW@",&now.to_string()).replace("@VOTING@",VOTING).replace("@EPOCH@",&epoch.to_string()).replace("@HASH@",&hash).replace("@BYTES@",&bytes)
}

#[test]
fn dao_verifies_identity_digest_dates_and_discussion_title() {
    let now = Utc::now().timestamp();
    let input = fixture(now);
    let feed: Feed = sonic_rs::from_str(&input).unwrap();
    let titles = BTreeMap::from([("7".into(), "Extend reward claims".into())]);
    let votes = feed.proposals(now, &titles).unwrap();
    assert_eq!(votes[0].title, "Extend reward claims");
    assert!(votes[0].open);
    assert!(votes[0].starts <= now && votes[0].ends > now);
    assert_eq!(
        votes[0].url,
        format!("https://dao.yearn.fi/proposals/0?chain=1&voting={VOTING}")
    );
    assert!(feed.proposals(now + 901, &titles).is_err());
    for (from, to) in [
        (VOTING, "0x0000000000000000000000000000000000000000"),
        ("yearn.dao.feed.v2", "unknown"),
        ("\"proposalCount\":\"1\"", "\"proposalCount\":\"2\""),
        ("0xabc", "0xabc"),
    ] {
        if from == to {
            continue;
        }
        let changed: Feed = sonic_rs::from_str(&input.replace(from, to)).unwrap();
        assert!(changed.proposals(now, &titles).is_err());
    }
    let mut changed = feed.clone();
    changed.proposals[0].content_digest = "0x00".into();
    assert!(changed.proposals(now, &titles).is_err());
    let mut missing = feed;
    missing.proposals[0].content_bytes = None;
    assert_eq!(
        missing.proposals(now, &titles).unwrap()[0].title,
        "Yearn proposal #0"
    );
}

#[test]
fn forum_links_are_bound_to_yearn_and_topic_id() {
    assert_eq!(
        discussion_identity("https://gov.yearn.fi/t/title/123/1"),
        Some("123".into())
    );
    assert_eq!(
        discussion_identity("https://evil.example/t/title/123"),
        None
    );
    assert_eq!(topic(123).url(), "https://gov.yearn.fi/t/123");
}

#[test]
fn noon_pacific_tracks_dst_with_23_and_25_hour_intervals() {
    for (month, day, hours) in [(3, 7, 23), (10, 31, 25)] {
        let a = Los_Angeles
            .with_ymd_and_hms(2026, month, day, 12, 0, 0)
            .unwrap();
        let b = (a.date_naive() + chrono::Duration::days(1))
            .and_hms_opt(12, 0, 0)
            .unwrap()
            .and_local_timezone(Los_Angeles)
            .unwrap();
        assert_eq!((b - a).num_hours(), hours);
    }
}

struct Server {
    url: String,
    requests: Arc<Mutex<Vec<String>>>,
    handle: thread::JoinHandle<()>,
}
impl Server {
    fn start(responses: Vec<(u16, String)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let saved = requests.clone();
        let handle = thread::spawn(move || {
            for (status, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    let n = stream.read(&mut chunk).unwrap();
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&buf[..end]).to_lowercase();
                        let len = headers
                            .lines()
                            .find_map(|l| {
                                l.strip_prefix("content-length: ")
                                    .and_then(|n| n.parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if buf.len() >= end + 4 + len {
                            break;
                        }
                    }
                }
                saved
                    .lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&buf).into_owned());
                write!(stream,"HTTP/1.1 {status} response\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
            }
        });
        Self {
            url,
            requests,
            handle,
        }
    }
    fn finish(self) -> Vec<String> {
        self.handle.join().unwrap();
        Arc::try_unwrap(self.requests)
            .unwrap()
            .into_inner()
            .unwrap()
    }
}

#[tokio::test]
async fn preview_fetches_real_schema_without_creating_files_or_publishing() {
    let now = Utc::now().timestamp();
    let dao = Server::start(vec![(200, fixture(now))]);
    let forum = Server::start(vec![(
        200,
        r#"{"topic_list":{"topics":[],"more_topics_url":null}}"#.into(),
    )]);
    let sources = Sources::new(
        &dao.url,
        &forum.url,
        "http://127.0.0.1:1",
        "yearn-votes".into(),
        None,
    )
    .unwrap();
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("absent.sqlite");
    let report = poll(&sources, path.clone(), true).await.unwrap();
    assert!(report.errors.is_empty());
    assert_eq!(report.notifications.len(), 1);
    assert!(!path.exists());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    assert_eq!(dao.finish().len(), 1);
    assert_eq!(forum.finish().len(), 1);
}

#[tokio::test]
async fn forum_pages_and_retries_are_bounded_and_deduplicated() {
    let first = r#"{"topic_list":{"topics":[{"id":1,"title":"One","slug":"one","created_at":"2026-11-01T00:00:00Z","visible":true,"archetype":"regular"}],"more_topics_url":"/next"}}"#;
    let last = r#"{"topic_list":{"topics":[],"more_topics_url":null}}"#;
    let server = Server::start(vec![
        (503, "down".into()),
        (200, first.into()),
        (200, last.into()),
    ]);
    let sources = Sources::new(
        &server.url,
        &server.url,
        &server.url,
        "yearn-votes".into(),
        None,
    )
    .unwrap();
    let topics = sources.forum().await.unwrap();
    assert_eq!(topics.len(), 1);
    let requests = server.finish();
    assert_eq!(requests.len(), 3);
    assert!(requests[2].contains("page=1"));
}

#[tokio::test]
async fn successful_receipt_required_and_token_never_in_payload() {
    let n = vote(true).notification("opened");
    let receipt = format!(
        r#"{{"event":"message","topic":"yearn-votes","id":"receipt","message":{}}}"#,
        sonic_rs::to_string(&n.message).unwrap()
    );
    let server = Server::start(vec![(200, receipt)]);
    let sources = Sources::new(
        &server.url,
        &server.url,
        &server.url,
        "yearn-votes".into(),
        Some("private-token".into()),
    )
    .unwrap();
    assert_eq!(sources.publish(&n).await.unwrap(), "receipt");
    let req = server.finish().pop().unwrap();
    assert!(req.contains("Bearer private-token"));
    let body = req.split("\r\n\r\n").nth(1).unwrap();
    assert!(!body.contains("private-token"));
    let server = Server::start(vec![(
        200,
        r#"{"event":"message","topic":"wrong","id":"x","message":"wrong"}"#.into(),
    )]);
    let sources = Sources::new(
        &server.url,
        &server.url,
        &server.url,
        "yearn-votes".into(),
        Some("token".into()),
    )
    .unwrap();
    assert!(sources.publish(&n).await.is_err());
    server.finish();
}

#[tokio::test]
async fn failed_dao_preserves_its_outbox_while_forum_delivers() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("state.sqlite");
    let state = State::load(path.clone(), false).await.unwrap();
    State::save(path.clone(), state.discussions(vec![], 100), 100)
        .await
        .unwrap();
    State::save(path.clone(), state.votes(vec![vote(false)], 100), 100)
        .await
        .unwrap();
    let dao = Server::start(vec![
        (503, "down".into()),
        (503, "down".into()),
        (503, "down".into()),
    ]);
    let forum = Server::start(vec![(200,r#"{"topic_list":{"topics":[{"id":2,"title":"New discussion","slug":"new","created_at":"2026-11-06T00:00:00Z","visible":true,"archetype":"regular"}],"more_topics_url":null}}"#.into())]);
    let ntfy = Server::start(vec![(200,r#"{"event":"message","topic":"yearn-votes","id":"forum-receipt","message":"New discussion"}"#.into())]);
    let sources = Sources::new(
        &dao.url,
        &forum.url,
        &ntfy.url,
        "yearn-votes".into(),
        Some("test-token".into()),
    )
    .unwrap();
    let report = poll(&sources, path.clone(), false).await.unwrap();
    assert_eq!(report.errors.len(), 1);
    assert!(report.errors[0].starts_with("dao:"));
    assert_eq!(report.delivered, 1);
    assert_eq!(State::pending(path, "dao".into()).await.unwrap().len(), 1);
    dao.finish();
    forum.finish();
    ntfy.finish();
}

#[tokio::test]
async fn rejected_delivery_remains_pending_after_database_reopen() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("state.sqlite");
    let dao = Server::start(vec![(200, fixture(Utc::now().timestamp()))]);
    let forum = Server::start(vec![(
        200,
        r#"{"topic_list":{"topics":[],"more_topics_url":null}}"#.into(),
    )]);
    let ntfy = Server::start(vec![(403, "denied".into())]);
    let sources = Sources::new(
        &dao.url,
        &forum.url,
        &ntfy.url,
        "yearn-votes".into(),
        Some("test-token".into()),
    )
    .unwrap();
    let report = poll(&sources, path.clone(), false).await.unwrap();
    assert_eq!(report.delivered, 0);
    assert_eq!(report.errors.len(), 1);
    assert_eq!(
        State::pending(path.clone(), "dao".into())
            .await
            .unwrap()
            .len(),
        1
    );
    let state = State::load(path.clone(), false).await.unwrap();
    assert_eq!(state.votes(vec![], 0).notifications.len(), 0);
    dao.finish();
    forum.finish();
    ntfy.finish();
}
