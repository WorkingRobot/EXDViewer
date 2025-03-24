FROM rust:alpine AS builder
USER root
WORKDIR /app
RUN apk add musl-dev
RUN cargo install --locked trunk

COPY . .
RUN cargo build --bin exdviewer-web --release

FROM alpine AS runtime
WORKDIR /app
COPY --from=builder /app/target/release/exdviewer-web exdviewer-web
COPY --from=builder /app/target/release/downloader downloader
COPY --from=builder /app/target/release/static static
CMD ["./exdviewer-web"]