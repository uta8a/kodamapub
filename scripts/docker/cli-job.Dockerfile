# syntax=docker/dockerfile:1.7
FROM rust:1.95-bookworm AS builder

RUN apt-get update \
  && apt-get install -y --no-install-recommends \
    ca-certificates \
    libsqlite3-dev \
    pkg-config \
    cmake \
  && rm -rf /var/lib/apt/lists/*

WORKDIR /workspace

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY apps ./apps

ENV CARGO_HOME=/usr/local/cargo
ENV RUST_BACKTRACE=1

RUN --mount=type=cache,id=kodamapub-cargo-registry,sharing=locked,target=/usr/local/cargo/registry \
  --mount=type=cache,id=kodamapub-cargo-git,sharing=locked,target=/usr/local/cargo/git \
  --mount=type=cache,id=kodamapub-cli-job-target,sharing=locked,target=/workspace/target \
  cargo build --locked --release -p kodamapub-cli \
  && cp /workspace/target/release/kodamapub /tmp/kodamapub-cli

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
  && apt-get install -y --no-install-recommends \
    ca-certificates \
    libsqlite3-0 \
  && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /tmp/kodamapub-cli /usr/local/bin/kodamapub-cli

ENTRYPOINT ["/usr/local/bin/kodamapub-cli"]
CMD []
