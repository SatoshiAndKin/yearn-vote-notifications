FROM rust:1.100.0-slim-bookworm@sha256:7c7d4eb4d05b7781187a327809466cfddf29753f6defdc3d19a0e291951b7a37 AS build
# Check the pinned image's compiler before rustup can see a project override.
COPY rust-toolchain.toml /tmp/yearn-rust-toolchain.toml
RUN --network=none set -eu; \
    expected="$(awk -F '"' '/^[[:blank:]]*channel[[:blank:]]*=/ { print $2 }' /tmp/yearn-rust-toolchain.toml)"; \
    actual="$(rustc --version --verbose | sed -n 's/^release: //p')"; \
    if [ -z "$expected" ] || [ "$actual" != "$expected" ]; then \
        printf 'Rust builder compiler mismatch: expected %s, got %s\n' "$expected" "$actual" >&2; \
        exit 1; \
    fi
WORKDIR /app
ENV CARGO_BUILD_JOBS=2
COPY Cargo.toml Cargo.lock ./
COPY src ./src
ARG BUILD_REVISION
RUN YEARN_BUILD_REVISION="$BUILD_REVISION" cargo build --release --locked
