use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    time::Duration,
};

use rusqlite::{Connection, OpenFlags, params};

use crate::{
    Result, discussion_notification,
    model::{Notification, Topic, Vote, invalid},
};

#[derive(Default)]
pub struct State {
    sources: BTreeSet<String>,
    seen: BTreeMap<String, BTreeSet<String>>,
    emitted: BTreeSet<String>,
    pending: Vec<Notification>,
}

pub struct Batch {
    pub source: String,
    pub entities: Vec<String>,
    pub active: BTreeSet<String>,
    pub notifications: Vec<Notification>,
}

fn connection(path: &PathBuf, readonly: bool) -> Result<Connection> {
    let conn = if readonly {
        Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?
    } else {
        Connection::open(path)?
    };
    conn.busy_timeout(Duration::from_secs(5))?;
    let version: u32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version > 1 {
        return Err(invalid("unsupported database version"));
    }
    if !readonly {
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            CREATE TABLE IF NOT EXISTS sources (name TEXT PRIMARY KEY, checked_at INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS seen (source TEXT NOT NULL, entity TEXT NOT NULL, PRIMARY KEY(source,entity));
            CREATE TABLE IF NOT EXISTS outbox (key TEXT PRIMARY KEY, source TEXT NOT NULL, entity TEXT NOT NULL,
                payload TEXT NOT NULL, receipt TEXT);
            CREATE TABLE IF NOT EXISTS health (id INTEGER PRIMARY KEY CHECK(id=1), last_success INTEGER NOT NULL);
            PRAGMA user_version=1;")?;
    }
    Ok(conn)
}

impl State {
    pub async fn load(path: PathBuf, readonly: bool) -> Result<Self> {
        tokio::task::spawn_blocking(move || {
            if readonly && !path.exists() {
                return Ok(Self::default());
            }
            let conn = connection(&path, readonly)?;
            let sources = conn
                .prepare("SELECT name FROM sources")?
                .query_map([], |r| r.get(0))?
                .collect::<std::result::Result<BTreeSet<String>, _>>()?;
            let mut seen: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
            for row in conn
                .prepare("SELECT source,entity FROM seen")?
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            {
                let (source, entity) = row?;
                seen.entry(source).or_default().insert(entity);
            }
            let mut emitted = BTreeSet::new();
            let mut pending = Vec::new();
            for row in conn
                .prepare("SELECT key,payload,receipt FROM outbox")?
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                    ))
                })?
            {
                let (key, payload, receipt) = row?;
                emitted.insert(key);
                if receipt.is_none() {
                    pending.push(sonic_rs::from_str(&payload)?);
                }
            }
            Ok(Self {
                sources,
                seen,
                emitted,
                pending,
            })
        })
        .await?
    }

    pub fn discussions(&self, topics: Vec<Topic>, _now: i64) -> Batch {
        let mut notifications: Vec<_> = self
            .pending
            .iter()
            .filter(|n| n.source == "forum")
            .cloned()
            .collect();
        let initialized = self.sources.contains("forum");
        for topic in &topics {
            if initialized
                && !self
                    .seen
                    .get("forum")
                    .is_some_and(|s| s.contains(&topic.id.to_string()))
            {
                let notification = discussion_notification(topic);
                if !self.emitted.contains(&notification.key) {
                    notifications.push(notification);
                }
            }
        }
        Batch {
            source: "forum".into(),
            entities: topics.iter().map(|t| t.id.to_string()).collect(),
            active: BTreeSet::new(),
            notifications,
        }
    }

    pub fn votes(&self, votes: Vec<Vote>, _now: i64) -> Batch {
        let active: BTreeSet<_> = votes
            .iter()
            .filter(|v| v.eligible)
            .map(|v| v.id.clone())
            .collect();
        let mut notifications: Vec<_> = self
            .pending
            .iter()
            .filter(|n| n.source == "dao" && active.contains(&n.entity))
            .cloned()
            .collect();
        for vote in &votes {
            if !vote.eligible {
                continue;
            }
            let publish = vote.notification("published");
            let opened = vote.notification("opened");
            if vote.open {
                // Publication and opening discovered together produce one push.
                notifications.retain(|n| n.key != publish.key);
                if !self.emitted.contains(&publish.key)
                    || self.pending.iter().any(|n| n.key == publish.key)
                {
                    notifications.push(Notification {
                        pending: false,
                        ..publish
                    });
                }
                if !self.emitted.contains(&opened.key) {
                    notifications.push(opened);
                }
            } else if !self.emitted.contains(&publish.key) {
                notifications.push(publish);
            }
        }
        Batch {
            source: "dao".into(),
            entities: votes.iter().map(|v| v.id.clone()).collect(),
            active,
            notifications,
        }
    }

    pub async fn save(path: PathBuf, batch: Batch, now: i64) -> Result<()> {
        tokio::task::spawn_blocking(move || {
            let mut conn = connection(&path, false)?;
            let tx = conn.transaction()?;
            tx.execute("INSERT INTO sources VALUES (?1,?2) ON CONFLICT(name) DO UPDATE SET checked_at=excluded.checked_at", params![batch.source,now])?;
            for entity in batch.entities { tx.execute("INSERT OR IGNORE INTO seen VALUES (?1,?2)", params![batch.source,entity])?; }
            if batch.source == "dao" {
                let pending = {
                    let mut stmt = tx.prepare("SELECT key,entity FROM outbox WHERE source='dao' AND receipt IS NULL")?;
                    stmt.query_map([], |r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<std::result::Result<Vec<_>,_>>()?
                };
                for (key,entity) in pending {
                    if !batch.active.contains(&entity) { tx.execute("UPDATE outbox SET receipt='inactive' WHERE key=?1",[key])?; }
                }
            }
            for n in batch.notifications {
                let receipt = (!n.pending).then_some("combined");
                tx.execute("INSERT INTO outbox VALUES (?1,?2,?3,?4,?5) ON CONFLICT(key) DO UPDATE SET payload=excluded.payload, receipt=COALESCE(outbox.receipt,excluded.receipt)",
                    params![n.key,n.source,n.entity,sonic_rs::to_string(&n)?,receipt])?;
            }
            tx.commit()?;
            Ok(())
        }).await?
    }

    pub async fn pending(path: PathBuf, source: String) -> Result<Vec<Notification>> {
        tokio::task::spawn_blocking(move || {
            let conn = connection(&path, true)?;
            let mut stmt = conn.prepare(
                "SELECT payload FROM outbox WHERE source=?1 AND receipt IS NULL ORDER BY key",
            )?;
            let rows = stmt.query_map([source], |r| r.get::<_, String>(0))?;
            let mut result = Vec::new();
            for row in rows {
                result.push(sonic_rs::from_str(&row?)?);
            }
            Ok(result)
        })
        .await?
    }

    pub async fn ack(path: PathBuf, key: String, receipt: String) -> Result<()> {
        tokio::task::spawn_blocking(move || {
            connection(&path, false)?.execute(
                "UPDATE outbox SET receipt=?2 WHERE key=?1",
                params![key, receipt],
            )?;
            Ok(())
        })
        .await?
    }

    pub async fn success(path: PathBuf, now: i64) -> Result<()> {
        tokio::task::spawn_blocking(move || {
            connection(&path,false)?.execute("INSERT INTO health VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET last_success=excluded.last_success",[now])?;
            Ok(())
        }).await?
    }
}
