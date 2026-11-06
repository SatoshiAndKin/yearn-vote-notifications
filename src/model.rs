use std::collections::{BTreeMap, BTreeSet};

use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Error, Result};

pub const VOTING: &str = "0x543e8871562a8c53e8b6a26835aeecb3a5a13070";
pub const GENESIS: i64 = 1_770_249_600;
pub const EPOCH_SECONDS: i64 = 14 * 24 * 60 * 60;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Notification {
    pub key: String,
    pub source: String,
    pub entity: String,
    pub title: String,
    pub message: String,
    pub click: String,
    pub expires_at: Option<i64>,
    pub pending: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Feed {
    pub schema: String,
    pub chain_id: u64,
    pub observed_at: i64,
    pub block: Block,
    pub deployments: Vec<Deployment>,
    pub proposals: Vec<Proposal>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Block {
    pub number: String,
    pub hash: String,
    pub timestamp: i64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Deployment {
    pub voting_address: String,
    pub proposal_count: String,
    pub configuration: Configuration,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Configuration {
    pub vote_start: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Proposal {
    pub voting_address: String,
    pub id: String,
    pub epoch: String,
    pub content_digest: String,
    pub content_bytes: Option<String>,
    pub status: String,
    pub retracted: bool,
    pub executed: bool,
    pub vetoed: bool,
    pub flagged: bool,
    pub events: Vec<Event>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Event {
    pub r#type: String,
    pub log: EventLog,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventLog {
    pub block_number: String,
    pub block_hash: String,
    pub timestamp: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Content {
    schema: String,
    markdown: String,
    discussion_url: String,
}

#[derive(Clone, Debug)]
pub struct Vote {
    pub id: String,
    pub title: String,
    pub url: String,
    pub starts: i64,
    pub ends: i64,
    pub open: bool,
    pub eligible: bool,
}

pub fn invalid(message: &str) -> Error {
    Error::Invalid(message.into())
}

fn number(value: &str) -> Result<i64> {
    value.parse().map_err(|_| invalid("invalid integer"))
}

impl Feed {
    pub fn proposals(&self, now: i64, titles: &BTreeMap<String, String>) -> Result<Vec<Vote>> {
        if self.schema != "yearn.dao.feed.v2" || self.chain_id != 1 {
            return Err(invalid("unknown DAO feed version or chain"));
        }
        if self.observed_at > now + 120
            || now - self.observed_at > 900
            || self.block.timestamp > self.observed_at
            || now - self.block.timestamp > 900
        {
            return Err(invalid("stale or future DAO feed"));
        }
        if self.deployments.len() != 1
            || self.deployments[0].voting_address.to_lowercase() != VOTING
        {
            return Err(invalid("unrecognized voting deployment"));
        }
        let deployment = &self.deployments[0];
        let count = number(&deployment.proposal_count)?;
        if count < 0 || count as usize != self.proposals.len() || count > 10_000 {
            return Err(invalid("incomplete proposal history"));
        }
        let offset = number(&deployment.configuration.vote_start)?;
        if !(0..EPOCH_SECONDS).contains(&offset) {
            return Err(invalid("invalid vote start offset"));
        }
        let mut seen = BTreeSet::new();
        let mut result = Vec::new();
        for p in &self.proposals {
            let id = number(&p.id)?;
            let epoch = number(&p.epoch)?;
            if id < 0
                || id >= count
                || !(0..=100_000).contains(&epoch)
                || !seen.insert(id)
                || p.voting_address.to_lowercase() != VOTING
            {
                return Err(invalid("invalid proposal identity"));
            }
            if p.events.first().is_none_or(|e| e.r#type != "propose") {
                return Err(invalid("missing proposal event"));
            }
            if !matches!(
                p.status.as_str(),
                "PROPOSED"
                    | "RETRACTED"
                    | "VOTING"
                    | "PASSED"
                    | "FAILED"
                    | "EXECUTED"
                    | "EXPIRED"
                    | "FLAGGED"
                    | "VETOED"
            ) {
                return Err(invalid("unknown proposal status"));
            }
            let starts = GENESIS + epoch * EPOCH_SECONDS + offset;
            let ends = GENESIS + (epoch + 1) * EPOCH_SECONDS;
            let eligible = !p.flagged
                && !p.retracted
                && !p.executed
                && !p.vetoed
                && now < ends
                && !matches!(
                    p.status.as_str(),
                    "FAILED" | "EXPIRED" | "PASSED" | "EXECUTED" | "VETOED" | "RETRACTED"
                );
            let open = eligible && p.status == "VOTING" && self.block.timestamp >= starts;
            let mut title = format!("Yearn proposal #{id}");
            if let Some(encoded) = &p.content_bytes {
                let bytes = STANDARD
                    .decode(encoded)
                    .map_err(|_| invalid("invalid proposal content encoding"))?;
                let digest = format!(
                    "0x{}",
                    Sha256::digest(&bytes)
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<String>()
                );
                if digest != p.content_digest.to_lowercase() {
                    return Err(invalid("proposal content digest mismatch"));
                }
                let content: Content = sonic_rs::from_slice(&bytes)?;
                if content.schema != "yearn.dao.proposal.v1" {
                    return Err(invalid("unknown proposal content version"));
                }
                if let Some(forum_title) =
                    titles.get(&discussion_identity(&content.discussion_url).unwrap_or_default())
                {
                    title = forum_title.clone();
                } else if let Some(heading) = content
                    .markdown
                    .lines()
                    .find_map(|line| line.strip_prefix("# "))
                    && !matches!(
                        heading.trim().to_lowercase().as_str(),
                        "summary" | "abstract"
                    )
                {
                    title = heading.trim().to_owned();
                }
            }
            result.push(Vote {
                id: format!("1:{VOTING}:{id}"),
                title,
                url: format!("https://dao.yearn.fi/proposals/{id}?chain=1&voting={VOTING}"),
                starts,
                ends,
                open,
                eligible,
            });
        }
        Ok(result)
    }
}

impl Vote {
    pub fn notification(&self, event: &str) -> Notification {
        let date = |ts| {
            DateTime::<Utc>::from_timestamp(ts, 0)
                .map(|d| d.format("%Y-%m-%d %H:%M UTC").to_string())
                .unwrap_or_default()
        };
        Notification {
            key: format!("dao:{}:{event}", self.id),
            source: "dao".into(),
            entity: self.id.clone(),
            title: if event == "opened" {
                "Yearn: voting open"
            } else {
                "Yearn: proposal published"
            }
            .into(),
            message: format!(
                "{}\nVoting: {} to {}",
                self.title,
                date(self.starts),
                date(self.ends)
            ),
            click: self.url.clone(),
            expires_at: Some(self.ends),
            pending: true,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Topic {
    pub id: u64,
    pub title: String,
    pub slug: String,
    pub created_at: DateTime<Utc>,
    pub visible: bool,
    pub archetype: String,
}

impl Topic {
    pub fn url(&self) -> String {
        // An ID-only link avoids trusting server-supplied URL or slug characters.
        format!("https://gov.yearn.fi/t/{}", self.id)
    }
}

#[derive(Debug, Deserialize)]
pub struct ForumPage {
    pub topic_list: TopicList,
}

#[derive(Debug, Deserialize)]
pub struct TopicList {
    pub topics: Vec<Topic>,
    pub more_topics_url: Option<String>,
}

pub fn discussion_identity(value: &str) -> Option<String> {
    let url = reqwest::Url::parse(value).ok()?;
    if url.scheme() != "https" || url.host_str()? != "gov.yearn.fi" {
        return None;
    }
    let parts: Vec<_> = url.path_segments()?.collect();
    if parts.first()? != &"t" {
        return None;
    }
    parts
        .iter()
        .skip(1)
        .find_map(|p| p.parse::<u64>().ok().map(|n| n.to_string()))
}
