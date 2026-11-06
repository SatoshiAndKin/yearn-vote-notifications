# Yearn vote notifications

A Rust and Tokio job that sends Yearn governance notifications to ntfy. K3s runs it once each day at **12:00 noon Pacific**, using `America/Los_Angeles` so the schedule follows daylight saving time.

## Alerts

The job reads Yearn's current `https://dao.yearn.fi/api/dao-data` feed and the Proposals category, including subcategories, at `https://gov.yearn.fi/c/proposals/5.json`.

It sends an alert for each new governance discussion, each published formal proposal, and each voting opening. When it first finds a proposal with voting already open, it sends one combined alert. Vote notifications link directly to the proposal on `dao.yearn.fi`. Discussion notifications link to the forum topic. Replies and edits do not trigger alerts.

First startup records existing discussions and completed votes without a backlog. It alerts for current and upcoming votes. Daily polling can delay detection by up to 24 hours. A vote that starts and ends between successful polls can be missed.

The built-in DAO deployment identity and timing follow Yearn's current mainnet Voting contract. An unknown deployment or feed version fails the DAO check so it can be reviewed before support is added. The job verifies content digests and rejects stale feed observations. Missing proposal content uses the proposal ID as its title.

## Run

Install the pinned Rust toolchain with rustup. Run these commands from the repository:

```sh
scripts/validate
cargo run --locked -- preview
NTFY_TOKEN_FILE=/path/to/token cargo run --locked -- --state /path/to/yearn.sqlite poll-once
NTFY_TOKEN_FILE=/path/to/token cargo run --locked -- test-notification
```

Preview fetches sources and shows proposed notifications. It does not create or change SQLite files, acquire a writer lock, send pushes, or send monitoring heartbeats.

The token file must contain an ntfy token for an account allowed to publish to the configured topic. Keep the file private. Do not pass a token on the command line.

| Variable | Default / purpose |
| --- | --- |
| `YEARN_STATE` | `yearn.sqlite`; parent directory must exist |
| `YEARN_DAO_URL` | Official DAO feed; override for local tests |
| `YEARN_FORUM_URL` | Official Proposals category JSON; override for local tests |
| `NTFY_BASE_URL` | `https://ntfy.stytt.com` |
| `NTFY_TOPIC` | `yearn-votes` |
| `NTFY_TOKEN_FILE` | Required for publishing; path to the token file |
| `HEALTHCHECK_URL_FILE` | Optional path to a private Uptime Kuma push URL |

Sources are fetched concurrently with bounded HTTP requests and retries. Each source has independent state. A failed source preserves its checkpoint while the other can still send alerts. SQLite operations run through Tokio `spawn_blocking`; network requests run outside transactions.

Pending notifications persist until ntfy returns a matching delivery receipt. Withdrawn, flagged, vetoed, and completed proposals suppress pending vote alerts. A process lock prevents concurrent writers. A crash after ntfy accepts a push but before its receipt is stored can cause a duplicate. Delivery is not exactly once.

The process returns nonzero for source, delivery, or heartbeat failures. It sends an Up heartbeat only after both sources and all deliveries succeed, or Down when a completed poll reports errors. The external monitor detects crashes and missing runs.

## Deployment

CI validates pull requests. Main builds publish an image tagged with the full commit to GHCR. Production uses its immutable image digest.

The private infrastructure controller owns the CronJob, retained PVC, restricted publisher Secret, monitoring, release records, and rollback. The schedule is:

```yaml
schedule: "0 12 * * *"
timeZone: America/Los_Angeles
concurrencyPolicy: Forbid
```

Each run starts the binary with `poll-once`, saves its state on the retained volume, and exits. The application has no listener and needs no wallet or transaction key. A replacement Pod reuses SQLite. Image rollback must preserve that volume and must never replace current state with an older copy.

Subscribe to **`https://ntfy.stytt.com/yearn-votes`** in the ntfy app with your existing reader account. A server delivery receipt proves ntfy acceptance. Confirm phone delivery and that tapping the notification opens the relevant Yearn page separately.
