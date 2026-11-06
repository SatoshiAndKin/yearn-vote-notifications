# Project rules

- Use Rust and Tokio. K3s owns the daily noon America/Los_Angeles schedule.
- Keep secrets and deployment host configuration out of this public repository.
- Run `scripts/validate` before committing. Commit Cargo.lock.
- Keep SQLite work off Tokio workers. Do not hold database transactions across await.
- Preview must not create or change state or send notifications.
- Use a feature branch and PR. No reviewer approval gate is required.
