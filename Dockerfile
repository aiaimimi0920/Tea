FROM rust:1.91.1-slim-bookworm AS builder

RUN apt-get update \
  && apt-get install -y --no-install-recommends build-essential ca-certificates libssl-dev pkg-config \
  && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY apps ./apps
COPY crates ./crates
COPY examples ./examples

RUN --mount=type=cache,target=/usr/local/cargo/registry \
  --mount=type=cache,target=/usr/local/cargo/git \
  cargo build --locked --release -p tea-daemon -p tea-cli

FROM debian:bookworm-slim

RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates libssl3 \
  && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/target/release/tea-daemon /usr/local/bin/tea-daemon
COPY --from=builder /app/target/release/tea-cli /usr/local/bin/tea

ENV TEA_BIND_ADDR=0.0.0.0:48910

EXPOSE 48910

CMD ["tea-daemon"]
