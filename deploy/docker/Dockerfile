# snaketui-server and snaketui-matchmaker. Build from the repository's root:
#   docker build -f deploy/docker/Dockerfile -t snaketui .
FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p snaketui-server --bins

FROM debian:bookworm-slim
# The game's directory belongs to its user, so a volume mounted there (or at
# key/, for a cluster's shared host key) starts out writable for it.
RUN useradd --system --home-dir /var/lib/snaketui snaketui \
    && mkdir -p /var/lib/snaketui/key \
    && chown -R snaketui:snaketui /var/lib/snaketui
COPY deploy/docker/server.toml /etc/snaketui/server.toml
COPY --from=build /src/target/release/snaketui-server /src/target/release/snaketui-matchmaker /usr/local/bin/
USER snaketui
# Players, and health and metrics.
EXPOSE 2222 9022
ENTRYPOINT ["snaketui-server"]
