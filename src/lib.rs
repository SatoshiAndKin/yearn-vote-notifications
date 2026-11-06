pub mod http;
pub mod model;
pub mod state;

use std::{collections::BTreeMap, path::PathBuf};

use chrono::Utc;
use http::Sources;
use model::{Notification, Topic};
use state::State;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid source data: {0}")]
    Invalid(String),
    #[error("HTTP request failed ({0})")]
    Http(String),
    #[error("database operation failed")]
    Database(#[from] rusqlite::Error),
    #[error("filesystem operation failed")]
    Io(#[from] std::io::Error),
    #[error("JSON could not be decoded")]
    Json(#[from] sonic_rs::Error),
    #[error("blocking database task failed")]
    Task(#[from] tokio::task::JoinError),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Default, serde::Serialize)]
pub struct Report {
    pub preview: bool,
    pub proposals: usize,
    pub discussions: usize,
    pub delivered: usize,
    pub notifications: Vec<Notification>,
    pub errors: Vec<String>,
}

pub async fn poll(sources: &Sources, state_path: PathBuf, preview: bool) -> Result<Report> {
    // The process lock covers fetch, reconcile and delivery, including manual runs.
    let _lock = if preview {
        None
    } else {
        let path = state_path.with_extension("lockfile");
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)?;
        lock.try_lock()
            .map_err(|_| Error::Invalid("another poll is running".into()))?;
        Some(lock)
    };
    let state = State::load(state_path.clone(), preview).await?;
    let (dao, forum) = tokio::join!(sources.dao(), sources.forum());
    let now = Utc::now().timestamp();
    let mut report = Report {
        preview,
        ..Report::default()
    };
    let topics = match forum {
        Ok(topics) => {
            report.discussions = topics.len();
            Some(topics)
        }
        Err(error) => {
            report.errors.push(format!("forum: {error}"));
            None
        }
    };
    let titles: BTreeMap<String, String> = topics
        .as_ref()
        .into_iter()
        .flatten()
        .map(|t| (t.id.to_string(), t.title.clone()))
        .collect();
    let mut batches = Vec::new();
    if let Some(topics) = topics {
        batches.push(state.discussions(topics, now));
    }
    match dao.and_then(|feed| feed.proposals(now, &titles)) {
        Ok(proposals) => {
            report.proposals = proposals.len();
            batches.push(state.votes(proposals, now));
        }
        Err(error) => report.errors.push(format!("dao: {error}")),
    }
    for batch in batches {
        let source = batch.source.clone();
        if preview {
            report
                .notifications
                .extend(batch.notifications.into_iter().filter(|n| n.pending));
            continue;
        }
        State::save(state_path.clone(), batch, now).await?;
        for notification in State::pending(state_path.clone(), source).await? {
            // Do not send an expired vote from a previously failed delivery.
            if notification
                .expires_at
                .is_some_and(|end| end <= Utc::now().timestamp())
            {
                State::ack(state_path.clone(), notification.key, "expired".into()).await?;
                continue;
            }
            match sources.publish(&notification).await {
                Ok(receipt) => {
                    State::ack(state_path.clone(), notification.key.clone(), receipt).await?;
                    report.delivered += 1;
                    report.notifications.push(notification);
                }
                Err(error) => report.errors.push(format!("delivery: {error}")),
            }
        }
    }
    if !preview && report.errors.is_empty() {
        State::success(state_path, now).await?;
    }
    Ok(report)
}

pub fn discussion_notification(topic: &Topic) -> Notification {
    Notification {
        key: format!("forum:{}:discussion", topic.id),
        source: "forum".into(),
        entity: topic.id.to_string(),
        title: "Yearn: new governance discussion".into(),
        message: topic.title.clone(),
        click: topic.url(),
        expires_at: None,
        pending: true,
    }
}
