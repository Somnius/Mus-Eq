# Build: docker build -f Containerfile -t museq-server .   (podman build works too)
FROM rust:1 AS build
# Keep the build under the CPU cap (cargo config on the host does not apply here).
ENV CARGO_BUILD_JOBS=20
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --release -p museq-server

FROM debian:stable-slim
COPY --from=build /src/target/release/museq-server /usr/local/bin/museq-server
RUN mkdir /data /music && chown 65534:65534 /data
ENV MUSEQ_BIND=0.0.0.0:8080 MUSEQ_DB=/data/museq.db MUSEQ_LIBRARY=/music
VOLUME ["/data"]
EXPOSE 8080
USER 65534:65534
ENTRYPOINT ["/usr/local/bin/museq-server"]
