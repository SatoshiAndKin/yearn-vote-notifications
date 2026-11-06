FROM rust:1.98.0-slim-bookworm@sha256:1469a27c125cb5a3aebfa4f4e4665d935b02fb72cc093b2c974b3d740e43f157 AS build
WORKDIR /app
ENV CARGO_BUILD_JOBS=2
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked

FROM debian:bookworm-slim@sha256:734a72b3d6142a71993b4bc580ca01d8ca687f77a2c422d96a961d3376b179bc
COPY --from=build /etc/ssl/certs/ /etc/ssl/certs/
COPY --from=build /app/target/release/yearn-vote-notifications /usr/local/bin/yearn-vote-notifications
USER 10001:10001
ENTRYPOINT ["/usr/local/bin/yearn-vote-notifications"]
CMD ["poll-once"]
