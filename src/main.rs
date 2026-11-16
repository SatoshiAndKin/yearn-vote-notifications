use std::{path::PathBuf, sync::Arc, time::Duration};

use clap::{Parser, Subcommand};
use eyre::{Result, eyre};
use sentry::{
    ClientInitGuard, Transport, TransportFactory, TransportOptions,
    integrations::tracing::ErrorLayer,
    transports::ReqwestHttpTransportOptions,
    types::{Dsn, Scheme},
};
use tracing_subscriber::{
    filter::{FilterExt, LevelFilter, filter_fn},
    prelude::*,
};
use yearn_vote_notifications::{http::Sources, model::Notification, poll};

#[derive(Parser)]
#[command(
    version,
    about = "Daily Yearn governance vote and discussion notifications"
)]
struct Args {
    #[arg(long, env = "YEARN_STATE", default_value = "yearn.sqlite")]
    state: PathBuf,
    #[arg(
        long,
        env = "YEARN_DAO_URL",
        default_value = "https://dao.yearn.fi/api/dao-data"
    )]
    dao_url: String,
    #[arg(
        long,
        env = "YEARN_FORUM_URL",
        default_value = "https://gov.yearn.fi/c/proposals/5.json"
    )]
    forum_url: String,
    #[arg(long, env = "NTFY_BASE_URL", default_value = "https://ntfy.stytt.com")]
    ntfy_base_url: String,
    #[arg(long, env = "NTFY_TOPIC", default_value = "yearn-votes")]
    ntfy_topic: String,
    #[arg(long, env = "NTFY_TOKEN_FILE")]
    ntfy_token_file: Option<PathBuf>,
    #[arg(long, env = "HEALTHCHECK_URL_FILE")]
    healthcheck_url_file: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    PollOnce,
    Preview,
    TestNotification,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let sentry = matches!(args.command, Command::PollOnce)
        .then(init_sentry)
        .flatten();
    run(args).await.inspect_err(|_| {
        if sentry.is_some() {
            tracing::error!("Yearn governance poll failed");
        }
    })
}

const SENTRY_TIMEOUT: Duration = Duration::from_secs(2);

struct TimedSentryTransport(reqwest::Client);

impl TimedSentryTransport {
    fn new(builder: reqwest::ClientBuilder) -> Result<Self, reqwest::Error> {
        builder
            .timeout(SENTRY_TIMEOUT)
            .https_only(true)
            .tls_danger_accept_invalid_certs(false)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map(Self)
    }
}

impl TransportFactory for TimedSentryTransport {
    fn create_transport_with_options(&self, options: TransportOptions) -> Arc<dyn Transport> {
        Arc::new(
            ReqwestHttpTransportOptions::from(options)
                .with_client(self.0.clone())
                .build(),
        )
    }
}

fn init_sentry() -> Option<ClientInitGuard> {
    let dsn = std::env::var("SENTRY_DSN")
        .ok()?
        .trim()
        .parse::<Dsn>()
        .ok()
        .filter(|dsn| dsn.scheme() == Scheme::Https)?;
    // The SDK joins its transport worker on shutdown. Bound the in-flight HTTP request too.
    let transport = TimedSentryTransport::new(reqwest::Client::builder()).ok()?;
    let guard = sentry::init(
        sentry::ClientOptions::from(dsn)
            .maybe_release(
                option_env!("YEARN_BUILD_REVISION")
                    .filter(|revision| !revision.is_empty())
                    .map(|revision| format!("{}@{revision}", env!("CARGO_PKG_NAME"))),
            )
            .default_integrations(false)
            .send_default_pii(false)
            .shutdown_timeout(SENTRY_TIMEOUT)
            .transport(transport),
    );
    // Send fixed diagnostics from the CLI. Keep structured error details in the job output.
    let diagnostics = filter_fn(|metadata| {
        metadata.target() == env!("CARGO_CRATE_NAME")
            && metadata.fields().len() == 1
            && metadata.fields().field("message").is_some()
    });
    tracing_subscriber::registry()
        .with(ErrorLayer::new().with_filter(LevelFilter::WARN.and(diagnostics)))
        .init();
    Some(guard)
}

async fn run(args: Args) -> Result<()> {
    let preview = matches!(args.command, Command::Preview);
    let token = if preview {
        None
    } else {
        let path = args
            .ntfy_token_file
            .ok_or_else(|| eyre!("set NTFY_TOKEN_FILE"))?;
        Some(tokio::fs::read_to_string(path).await?.trim().to_owned())
    };
    let sources = Sources::new(
        &args.dao_url,
        &args.forum_url,
        &args.ntfy_base_url,
        args.ntfy_topic,
        token,
    )?;
    if matches!(args.command, Command::TestNotification) {
        let receipt = sources.publish(&Notification {
            key:"test".into(),source:"test".into(),entity:"test".into(),
            title:"Yearn notifications: delivery test".into(),
            message:"Test notification. Daily checks run at noon Pacific. Tap to open Yearn governance.".into(),
            click:"https://dao.yearn.fi/".into(),expires_at:None,pending:true,
        }).await?;
        println!("{}", sonic_rs::to_string(&receipt)?);
        return Ok(());
    }
    let report = poll(&sources, args.state, preview).await?;
    println!("{}", sonic_rs::to_string(&report)?);
    if !preview && let Some(path) = args.healthcheck_url_file {
        let endpoint = tokio::fs::read_to_string(path).await?;
        sources
            .heartbeat(endpoint.trim(), report.errors.is_empty())
            .await?;
    }
    if !report.errors.is_empty() {
        return Err(eyre!(
            "poll incomplete; see source and delivery errors above"
        ));
    }
    Ok(())
}
