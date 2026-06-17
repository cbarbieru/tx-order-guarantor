# CLAUDE.md

Guidance for Claude Code working in this repository. See [README.md](README.md)
for the full architecture, SGX-host setup, and RA-TLS details.

## What this is

A **transaction order guarantor (TOG)** for Optimism/L2, refactored so the
mempool **ordering runs inside an Intel SGX enclave** (Fortanix EDP) as the
smallest trusted unit. Clients submit raw txs over **RA-TLS terminating inside
the enclave**; the builder reads back the raw txs and the enclave-computed
ordering. Everything else (passthrough `eth_*` → builder) stays outside.

This is a **Cargo virtual workspace** — there is no root package. The earlier
monolithic `tx-order-guarantor` crate (reth `OpTransactionPool` + jsonrpsee in
one binary) has been removed; do not reintroduce reth into the enclave path (its
C deps — c-kzg, secp256k1-C, mdbx — won't compile for `x86_64-fortanix-unknown-sgx`).

## Crates

- **crates/tog-proto** — length-prefixed JSON wire protocol (3 ops). Pure, no C.
- **crates/tog-core** — the TCB: decode + signer recovery (pure-Rust `k256`) +
  gap-aware, replace-by-fee, tip-priority ordering. Unit-tested. No state access.
- **crates/tog-enclave** — SGX binary. No Tokio (blocking `std::net` +
  thread-per-conn). `src/transport.rs` holds the RA-TLS/Enclave-Manager wiring
  under `cfg(target_env = "sgx")`; the host build is a plaintext dev server.
- **crates/tog-host** — untrusted `eth_*` passthrough proxy (tokio + jsonrpsee).
- **crates/tog-client** — test/reference client; RA-TLS verifier behind the
  `ratls` feature.

## Commands

```bash
cargo test -p tog-proto -p tog-core     # the logic that matters (verifiable on any host)
cargo build                             # whole workspace (host targets)
cargo run  -p tog-enclave               # PLAINTEXT dev server on :1546 (not attested)
cargo run  -p tog-host                  # passthrough proxy on :1545
cargo run  -p tog-client -- --addr 127.0.0.1:1546 demo   # end-to-end smoke test
cargo clippy && cargo fmt

# Enclave build (Linux SGX host, NIGHTLY — see README "Build-host setup"):
rustup target add x86_64-fortanix-unknown-sgx --toolchain nightly
cargo +nightly build --release -p tog-enclave --target x86_64-fortanix-unknown-sgx
```

## Constraints to remember

- **Apple-Silicon Macs cannot build for SGX or run enclaves** (no HW, no
  simulator). The plaintext dev path + `tog-core` tests work there; the SGX path
  is `cfg`-gated so host builds stay green.
- **Fortanix EDP requires Rust nightly.**
- Enclave env vars: `TOG_ENCLAVE_BIND` (default `0.0.0.0:1546`), `TOG_BASE_FEE`;
  RA-TLS path also `NODE_AGENT_URL` + `TOG_TLS_CN`. Host: `BUILDER_HOST`,
  `BUILDER_PORT`, `TOG_HOST_BIND`.
- The enclave holds no chain state and an ephemeral pool (lost on restart;
  in-enclave time is untrusted).
