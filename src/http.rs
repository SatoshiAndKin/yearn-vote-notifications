use std::{collections::BTreeSet, time::Duration};

use reqwest::{Client, Method, Url};
use serde::{Deserialize, Serialize};
use tokio::time::sleep;

use crate::{
    Error, Result,
    model::{Feed, ForumPage, Notification, Topic, invalid},
};

pub struct Sources {
    client: Client,
    pub dao_url: Url,
    pub forum_url: Url,
    pub ntfy_url: Url,
    pub topic: String,
    token: Option<String>,
}

impl Sources {
    pub fn new(
        dao: &str,
        forum: &str,
        ntfy: &str,
        topic: String,
        token: Option<String>,
    ) -> Result<Self> {
        fn url(value: &str) -> Result<Url> {
            let url = Url::parse(value).map_err(|_| invalid("invalid HTTP URL"))?;
            if !matches!(url.scheme(), "http" | "https")
                || !url.username().is_empty()
                || url.password().is_some()
                || url.fragment().is_some()
            {
                return Err(invalid(
                    "URLs must be HTTP(S) without credentials or fragments",
                ));
            }
            Ok(url)
        }
        if topic.is_empty()
            || !topic
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(invalid("invalid ntfy topic"));
        }
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!(
                env!("CARGO_PKG_NAME"),
                "/",
                env!("CARGO_PKG_VERSION")
            ))
            .build()
            .map_err(|_| Error::Http("client setup".into()))?;
        Ok(Self {
            client,
            dao_url: url(dao)?,
            forum_url: url(forum)?,
            ntfy_url: url(ntfy)?,
            topic,
            token,
        })
    }

    async fn request(
        &self,
        method: Method,
        url: Url,
        body: Option<Vec<u8>>,
        limit: usize,
    ) -> Result<Vec<u8>> {
        for attempt in 0..3 {
            let mut request = self.client.request(method.clone(), url.clone());
            if let Some(body) = &body {
                let token = self
                    .token
                    .as_ref()
                    .ok_or_else(|| invalid("missing ntfy token file"))?;
                request = request
                    .bearer_auth(token)
                    .header("Content-Type", "application/json")
                    .body(body.clone());
            } else {
                request = request.header("Cache-Control", "no-cache");
            }
            let result = request.send().await;
            match result {
                Ok(mut response) if response.status().is_success() => {
                    if response.content_length().is_some_and(|n| n > limit as u64) {
                        return Err(invalid("HTTP response exceeds size limit"));
                    }
                    let mut bytes = Vec::new();
                    while let Some(chunk) = response
                        .chunk()
                        .await
                        .map_err(|_| Error::Http("response body".into()))?
                    {
                        if bytes.len() + chunk.len() > limit {
                            return Err(invalid("HTTP response exceeds size limit"));
                        }
                        bytes.extend_from_slice(&chunk);
                    }
                    return Ok(bytes);
                }
                Ok(response) => {
                    let status = response.status();
                    if attempt == 2 || !(status.is_server_error() || status.as_u16() == 429) {
                        return Err(Error::Http(status.as_u16().to_string()));
                    }
                }
                Err(_) if attempt == 2 => {
                    return Err(Error::Http(
                        "transport timeout or connection failure".into(),
                    ));
                }
                Err(_) => (),
            }
            sleep(Duration::from_secs(1 << attempt)).await;
        }
        Err(Error::Http("retry limit".into()))
    }

    pub async fn dao(&self) -> Result<Feed> {
        Ok(sonic_rs::from_slice(
            &self
                .request(Method::GET, self.dao_url.clone(), None, 32 * 1024 * 1024)
                .await?,
        )?)
    }

    pub async fn forum(&self) -> Result<Vec<Topic>> {
        let mut topics = Vec::new();
        let mut seen = BTreeSet::new();
        for page in 0..100 {
            let mut url = self.forum_url.clone();
            url.query_pairs_mut()
                .append_pair("order", "created")
                .append_pair("ascending", "false")
                .append_pair("page", &page.to_string());
            let result: ForumPage = sonic_rs::from_slice(
                &self
                    .request(Method::GET, url, None, 2 * 1024 * 1024)
                    .await?,
            )?;
            if result.topic_list.topics.is_empty() {
                return Ok(topics);
            }
            let before = seen.len();
            for topic in result.topic_list.topics {
                if seen.insert(topic.id) && topic.visible && topic.archetype == "regular" {
                    topics.push(topic);
                }
            }
            if result.topic_list.more_topics_url.is_none() {
                return Ok(topics);
            }
            if before == seen.len() {
                return Err(invalid("forum pagination did not advance"));
            }
        }
        Err(invalid("forum pagination exceeds limit"))
    }

    pub async fn heartbeat(&self, endpoint: &str, success: bool) -> Result<()> {
        let mut url = Url::parse(endpoint).map_err(|_| invalid("invalid heartbeat URL"))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(invalid("invalid heartbeat scheme"));
        }
        url.query_pairs_mut()
            .append_pair("status", if success { "up" } else { "down" })
            .append_pair(
                "msg",
                if success {
                    "Yearn daily poll completed"
                } else {
                    "Yearn daily poll failed"
                },
            );
        #[derive(Deserialize)]
        struct Heartbeat {
            ok: bool,
        }
        let result: Heartbeat =
            sonic_rs::from_slice(&self.request(Method::GET, url, None, 65536).await?)?;
        if !result.ok {
            return Err(invalid("heartbeat rejected"));
        }
        Ok(())
    }

    pub async fn publish(&self, notification: &Notification) -> Result<String> {
        #[derive(Serialize)]
        struct Message<'a> {
            topic: &'a str,
            title: &'a str,
            message: &'a str,
            click: &'a str,
            priority: u8,
        }
        #[derive(Deserialize)]
        struct Receipt {
            id: String,
            topic: String,
            event: String,
            message: String,
        }
        let payload = sonic_rs::to_vec(&Message {
            topic: &self.topic,
            title: &notification.title,
            message: &notification.message,
            click: &notification.click,
            priority: 3,
        })?;
        let receipt: Receipt = sonic_rs::from_slice(
            &self
                .request(Method::POST, self.ntfy_url.clone(), Some(payload), 65536)
                .await?,
        )?;
        if receipt.id.is_empty()
            || receipt.topic != self.topic
            || receipt.event != "message"
            || receipt.message != notification.message
        {
            return Err(invalid("unexpected ntfy delivery receipt"));
        }
        Ok(receipt.id)
    }
}
