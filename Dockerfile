FROM rust:1-slim-trixie AS builder

WORKDIR /app

RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
RUN mkdir src \
    && echo 'fn main() {}' > src/main.rs \
    && touch src/lib.rs \
    && cargo build --release --locked \
    && rm -rf src

COPY src ./src
RUN touch src/main.rs src/lib.rs \
    && cargo build --release --locked

FROM debian:trixie-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/mcim-rust-sync /usr/local/bin/mcim-rust-sync

RUN useradd --system --user-group --home-dir /app --shell /usr/sbin/nologin mcim \
    && mkdir -p /app \
    && chown mcim:mcim /app

WORKDIR /app
USER mcim

ENTRYPOINT ["mcim-rust-sync"]
CMD ["daemon"]
