# Cross-compilation images — selected by TARGETARCH (set automatically by buildx)
ARG TARGETARCH=amd64
FROM --platform=$BUILDPLATFORM messense/rust-musl-cross:x86_64-musl AS cross-amd64
FROM --platform=$BUILDPLATFORM messense/rust-musl-cross:aarch64-musl AS cross-arm64
FROM cross-${TARGETARCH} AS builder

WORKDIR /build
COPY . .

ARG TARGETARCH
RUN --mount=type=secret,id=git_auth,env=GIT_AUTH_URL \
    if [ -n "$GIT_AUTH_URL" ]; then git config --global url."$GIT_AUTH_URL".insteadOf "https://github.com/"; fi && \
    RUST_TARGET=$(if [ "$TARGETARCH" = "arm64" ]; then echo "aarch64-unknown-linux-musl"; else echo "x86_64-unknown-linux-musl"; fi) && \
    cargo build --release --target "$RUST_TARGET" \
    -p shroudb-courier-server -p shroudb-courier-cli && \
    mkdir -p /out && \
    cp "target/$RUST_TARGET/release/shroudb-courier" /out/ && \
    cp "target/$RUST_TARGET/release/shroudb-courier-cli" /out/

# --- shroudb-courier: secure notification delivery pipeline ---
FROM alpine:3.21 AS shroudb-courier
RUN adduser -D -u 65532 shroudb && \
    mkdir /data && chown shroudb:shroudb /data
LABEL org.opencontainers.image.title="ShrouDB Courier" \
      org.opencontainers.image.description="Secure notification delivery pipeline — decrypts Transit-encrypted recipients, renders templates, delivers via adapter" \
      org.opencontainers.image.vendor="ShrouDB" \
      org.opencontainers.image.url="https://github.com/shroudb/shroudb-courier" \
      org.opencontainers.image.source="https://github.com/shroudb/shroudb-courier" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0"
COPY --from=builder /out/shroudb-courier /shroudb-courier
VOLUME /data
WORKDIR /data
USER shroudb
EXPOSE 6999 7000
ENTRYPOINT ["/shroudb-courier"]

# --- shroudb-courier-cli: interactive CLI ---
FROM alpine:3.21 AS shroudb-courier-cli
RUN adduser -D -u 65532 shroudb
LABEL org.opencontainers.image.title="ShrouDB Courier CLI" \
      org.opencontainers.image.description="Interactive CLI for ShrouDB Courier" \
      org.opencontainers.image.vendor="ShrouDB" \
      org.opencontainers.image.url="https://github.com/shroudb/shroudb-courier" \
      org.opencontainers.image.source="https://github.com/shroudb/shroudb-courier" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0"
COPY --from=builder /out/shroudb-courier-cli /shroudb-courier-cli
USER shroudb
ENTRYPOINT ["/shroudb-courier-cli"]
