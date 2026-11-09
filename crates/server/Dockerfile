# Build from the repo root:  docker build -f crates/server/Dockerfile -t jotbar-server .
FROM rust:1-slim AS build
WORKDIR /src
COPY . .
# The Tauri app needs GTK/WebKit to build, so only build the server crate.
RUN cargo build --release -p jotbar-server

FROM debian:stable-slim
COPY --from=build /src/target/release/jotbar-server /usr/local/bin/jotbar-server
ENV JOTBAR_DATA=/data JOTBAR_ADDR=0.0.0.0:8787
VOLUME /data
EXPOSE 8787
USER 1000:1000
CMD ["jotbar-server"]
