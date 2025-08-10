# Builder stage (same as before)
FROM rust:1.88 as builder

RUN apt-get update && apt-get install -y clang libclang-dev pkg-config build-essential

WORKDIR /usr/src/app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo fetch
RUN cargo build --release

# Runtime stage
FROM debian:buster-slim
# RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

WORKDIR /usr/local/bin

# Copy binary
COPY --from=builder /usr/src/app/target/release/tx-order-guarantor .

# Copy the `res` folder **two levels up** relative to WORKDIR
COPY res /usr/res

# Set working directory to where the binary expects to be run from
# so that "../../res/l2-genesis.json" resolves to /usr/res/l2-genesis.json
WORKDIR /usr/local/bin

CMD ["./tx-order-guarantor"]
