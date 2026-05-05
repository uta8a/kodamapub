FROM rust:1.95-bookworm

RUN apt-get update \
  && apt-get install -y --no-install-recommends \
    ca-certificates \
    libsqlite3-dev \
    pkg-config \
  && rm -rf /var/lib/apt/lists/*

WORKDIR /workspace

ENV CARGO_HOME=/usr/local/cargo
ENV RUST_BACKTRACE=1

CMD ["cargo", "run", "-p", "kodamapub-server"]
