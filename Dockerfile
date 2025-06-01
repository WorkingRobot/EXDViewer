FROM rust:alpine AS builder
USER root
WORKDIR /app
RUN rustup target add wasm32-unknown-unknown
RUN apk add musl-dev trunk dotnet9-sdk git

RUN git clone --depth 1 --filter=blob:none --no-checkout https://github.com/WorkingRobot/ffxiv-downloader.git
RUN git -C ffxiv-downloader sparse-checkout init --no-cone 
RUN echo '/*' > ffxiv-downloader/.git/info/sparse-checkout
RUN echo '!/luts/' >> ffxiv-downloader/.git/info/sparse-checkout
RUN echo '!/cluts/' >> ffxiv-downloader/.git/info/sparse-checkout
RUN git -C ffxiv-downloader checkout
ARG DOTNET_CLI_TELEMETRY_OPTOUT=1
RUN dotnet publish --nologo -c Release -o downloader-build -p:PublishSingleFile=true --self-contained false ffxiv-downloader/FFXIVDownloader.Command

COPY . .
RUN cargo build --bin web --release

FROM alpine AS runtime
WORKDIR /app
RUN apk add dotnet9-runtime
COPY --from=builder /app/target/release/web web
COPY --from=builder /app/downloader-build/FFXIVDownloader.Command downloader
COPY --from=builder /app/target/release/static static
CMD ["./web"]