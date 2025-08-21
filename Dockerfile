FROM rust:alpine AS builder
USER root
WORKDIR /app
RUN rustup target add wasm32-unknown-unknown
RUN apk add musl-dev trunk openssl-dev openssl-libs-static

COPY . .
RUN cargo build --bin web --release --features trunk_assets

FROM alpine AS runtime
WORKDIR /app
COPY --from=builder /app/target/release/web web
COPY --from=builder /app/target/release/static static
CMD ["./web"]
