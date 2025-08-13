# Builder stage (same as before)
FROM rust:1.88 as builder

RUN apt-get update && apt-get install -y clang libclang-dev pkg-config build-essential
RUN apt-get update && apt-get install -y musl-tools
RUN rustup target add x86_64-unknown-linux-musl

WORKDIR /usr/src/app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
# RUN cargo fetch
RUN cargo build --release --target x86_64-unknown-linux-musl

# Runtime stage
FROM scratch
# RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

WORKDIR /usr/local/bin

# Copy binary
COPY --from=builder /usr/src/app/target/x86_64-unknown-linux-musl/release/tx-order-guarantor .

COPY res /usr/local/bin/res

CMD ["./tx-order-guarantor"]
