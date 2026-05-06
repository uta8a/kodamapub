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
COPY scripts ./scripts

ENV CARGO_HOME=/usr/local/cargo
ENV RUST_BACKTRACE=1

RUN cargo build --locked --release -p kodamapub-cli

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
  && apt-get install -y --no-install-recommends \
    ca-certificates \
    libsqlite3-0 \
  && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /workspace/target/release/kodamapub /usr/local/bin/kodamapub-cli

ENTRYPOINT ["/usr/local/bin/kodamapub-cli"]
CMD []
