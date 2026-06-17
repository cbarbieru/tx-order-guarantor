# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```bash
cargo build                    # Debug build
cargo build --release          # Optimized release build
cargo run                      # Run the server locally
cargo test                     # Run tests
cargo clippy                   # Lint
cargo fmt                      # Format code

# Cross-compile for Docker (Linux musl)
cargo build --release --target x86_64-unknown-linux-musl
```

The server listens on `0.0.0.0:1545` by default. Builder connection is controlled via env vars:
- `BUILDER_HOST` (default: `"op-rbuilder"`)
- `BUILDER_PORT` (default: `"8545"`)

## Architecture

This is a **transaction order guarantor (TOG)** — a JSON-RPC proxy that sits between transaction submitters and a builder node for Optimism/L2. It accepts raw transactions, maintains an ordered pool, and exposes custom RPC methods for the builder to retrieve ordered batches.

### Source files

- **[src/main.rs](src/main.rs)** — Loads L2 genesis from `res/l2-genesis.json`, builds the Reth `OpTransactionPool`, starts the `jsonrpsee` HTTP server, and composes RPC modules.
- **[src/rpc.rs](src/rpc.rs)** — `GuarantorApi` with the core RPC surface. Custom endpoints: `tog_getRawTransactions` and `tog_getBestTransactionHashes`. Passthrough eth_* methods forward to the builder via HTTP client.
- **[src/noop.rs](src/noop.rs)** — `NoopProviderTog`: a mock Reth storage/state provider that returns empty defaults. Contains a nonce callback mechanism so the pool can resolve account nonces without a real chain backend.

### Transaction flow

1. Client submits raw tx via `eth_sendRawTransaction`.
2. TOG decodes, validates signature, and adds to `OpTransactionPool` (ordered by `CoinbaseTipOrdering`).
3. Raw bytes are also stored in a side buffer.
4. Builder calls `tog_getBestTransactionHashes` → returns hash-ordered list; pool clears every 7 calls.
5. Builder calls `tog_getRawTransactions` → returns serialized bytes; buffer clears on read.

### Key configuration

- Pool limits: 100,000 txs / 512 MB per sub-pool (pending, queued, basefee).
- Server limits: 10 MB max request/response, 1,000 max concurrent connections.
- No blob store persistence (`NoopBlobStore`).

### Dependencies

Uses the **Reth v1.6.0** ecosystem (`reth-optimism-txpool`, `reth-optimism-chainspec`, `reth-optimism-rpc`) and **Alloy** for Ethereum types. JSON-RPC layer is `jsonrpsee`; async runtime is Tokio.
