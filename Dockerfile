# 第一阶段：构建阶段
FROM rust:1-slim-trixie AS builder

WORKDIR /app

# 先只带依赖清单进来单独构建一次，源码改动时这一层还能命中缓存
COPY Cargo.toml Cargo.lock ./
RUN mkdir src \
    && echo 'fn main() {}' > src/main.rs \
    && touch src/lib.rs \
    && cargo build --release --locked \
    && rm -rf src

COPY src ./src
# COPY 保留的是源文件 mtime，可能比上一层的产物还旧，
# 不 touch 的话 cargo 会认为没有改动，直接把空壳二进制发出去
RUN touch src/main.rs src/lib.rs \
    && cargo build --release --locked

# 第二阶段：运行阶段
FROM debian:trixie-slim

# 走的是 rustls，不需要 OpenSSL，但根证书不能少，
# 否则所有 HTTPS 请求都会失败
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/mcim-rust-sync /usr/local/bin/mcim-rust-sync

# 不用 root 跑。注意挂进来的 config.json 要让这个用户读得到
RUN useradd --system --user-group --home-dir /app --shell /usr/sbin/nologin mcim \
    && mkdir -p /app \
    && chown mcim:mcim /app

WORKDIR /app
USER mcim

# 一个二进制两种用法，靠覆盖 CMD 切换：
#   docker run -d   镜像                    常驻，按 config.json 里的 schedule 调度
#   docker run --rm 镜像 modrinth refresh   跑完即退，退出码 0/1/2
ENTRYPOINT ["mcim-rust-sync"]
CMD ["daemon"]
