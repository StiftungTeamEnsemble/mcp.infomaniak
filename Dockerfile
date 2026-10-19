FROM node:24-bookworm-slim AS ui
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci
COPY web ./web
RUN npm run build

FROM rust:1.97-bookworm AS rust
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY web ./web
RUN cargo build --locked --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/* && useradd --uid 10001 --create-home mcp && mkdir /data && chown mcp:mcp /data
WORKDIR /app
COPY --from=rust /app/target/release/ensemble-mcp /usr/local/bin/ensemble-mcp
COPY --from=ui /app/web/dist ./web/dist
USER mcp
ENV BIND_ADDR=0.0.0.0:3000 SQLITE_PATH=/data/mcp.sqlite
EXPOSE 3000
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s CMD ["ensemble-mcp", "healthcheck"]
ENTRYPOINT ["ensemble-mcp"]
