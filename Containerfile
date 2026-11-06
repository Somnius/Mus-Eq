# Build: docker build -f Containerfile -t museq-server .   (podman build works too)
FROM node:22-bookworm-slim AS web
WORKDIR /web
COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY web/ ./
# esbuild/rolldown spawn one worker per core; keep the build small
ENV UV_THREADPOOL_SIZE=4
RUN npm run build

FROM rust:1 AS build
# Keep the build under the CPU cap (cargo config on the host does not apply here).
ENV CARGO_BUILD_JOBS=20
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --release -p museq-server

FROM debian:stable-slim
COPY --from=build /src/target/release/museq-server /usr/local/bin/museq-server
COPY --from=web /web/dist /usr/share/museq/web
COPY data/stations /usr/share/museq/stations
RUN mkdir /data /music && chown 65534:65534 /data
ENV MUSEQ_BIND=0.0.0.0:8080 MUSEQ_DB=/data/museq.db MUSEQ_LIBRARY=/music MUSEQ_SKINS_DIR=/data/skins MUSEQ_WEB_DIR=/usr/share/museq/web MUSEQ_STATIONS_SEED=/usr/share/museq/stations/museq-stations.txt,/usr/share/museq/stations/trayfm-stations.txt
VOLUME ["/data"]
EXPOSE 8080
USER 65534:65534
ENTRYPOINT ["/usr/local/bin/museq-server"]
