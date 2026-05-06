# syntax=docker/dockerfile:1.7
FROM rust:1.95-bookworm AS builder

RUN apt-get update \
  && apt-get install -y --no-install-recommends \
    ca-certificates \
    pkg-config \
    libssl-dev \
    cmake \
  && rm -rf /var/lib/apt/lists/*

WORKDIR /workspace

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY apps ./apps

ENV CARGO_HOME=/usr/local/cargo
ENV RUST_BACKTRACE=1

RUN --mount=type=cache,target=/usr/local/cargo/registry \
  --mount=type=cache,target=/usr/local/cargo/git \
  --mount=type=cache,target=/workspace/target \
  cargo build --locked --release -p kodamapub-edge

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
  && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl-dev \
  && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /workspace/target/release/kodamapub-edge /usr/local/bin/kodamapub-edge

ENV EDGE_LISTEN_ADDR=0.0.0.0:8080

EXPOSE 8080

CMD ["/usr/local/bin/kodamapub-edge"]
