FROM rust:1.88-alpine AS builder

WORKDIR /build
RUN apk add --no-cache musl-dev
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo test --locked --release && cargo build --locked --release

FROM scratch

COPY --from=builder /build/target/release/komodo-mail-bridge /komodo-mail-bridge

USER 65532:65532
EXPOSE 8000
ENTRYPOINT ["/komodo-mail-bridge"]
