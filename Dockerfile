# kkt in a container. Build from the repository root:
#   docker build -t kkt .
# The database, access key, web key and config live under /data: mount one volume there.

FROM rust:1-bookworm AS build
WORKDIR /src
# The whole tree, .git included, so build.rs can record the commit.
COPY . .
RUN cargo build --release --locked --bin kkt

FROM debian:bookworm-slim
RUN useradd --system --uid 10001 --home-dir /data --shell /usr/sbin/nologin kkt \
    && mkdir /data \
    && chown kkt:kkt /data
COPY --from=build /src/target/release/kkt /usr/local/bin/kkt
USER kkt
# kkt finds its database, key and config file under $HOME.
ENV HOME=/data
VOLUME /data
EXPOSE 7420
# Inside a container kkt must listen on the container's own interface. The
# access key stays on, which kkt requires for this. Publish the port to
# 127.0.0.1 only (see compose.yaml).
ENTRYPOINT ["kkt"]
CMD ["serve", "--bind", "0.0.0.0:7420", "--allow-any-bind"]
