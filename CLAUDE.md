# CLAUDE.md

Guidance for Claude Code working in this repository. See [README.md](README.md)
for the full architecture and SGX-host setup.

## What this is

A **transaction order guarantor (TOG)** for Optimism/L2, refactored so the
mempool **ordering runs inside an Intel SGX enclave** (Fortanix EDP) as the
smallest trusted unit. Clients submit raw txs **directly to the enclave** and the
builder reads back the raw txs and the enclave-computed ordering. There is
**deliberately no proxy in the transaction path** (censorship resistance):
clients query the builder directly for read-only `eth_*`.

**Attestation is currently a dev stub**: the enclave **always** opens a connection
with a fake attestation frame, and the client always reads it (no flag, no env —
EDP enclaves don't inherit the host environment, so it's baked into the build).
The transport is **plaintext on all targets**, including inside a real enclave. A
real attested transport is future work — do **not** re-add an Enclave-Manager /
RA-TLS path unless asked.

This is a **Cargo virtual workspace** — there is no root package. Do not
reintroduce reth into the enclave path (its C deps — c-kzg, secp256k1-C, mdbx —
won't compile for `x86_64-fortanix-unknown-sgx`).

## Crates

- **crates/tog-proto** — length-prefixed JSON wire protocol (3 ops + the dev stub
  attestation frame). Pure, no C.
- **crates/tog-core** — the TCB: decode + signer recovery (pure-Rust `k256`) +
  gap-aware, replace-by-fee, tip-priority ordering. Unit-tested. No state access.
- **crates/tog-enclave** — SGX binary. No Tokio (blocking `std::net` +
  thread-per-conn). `src/transport.rs` is the (plaintext) transport seam.
- **crates/tog-client** — test/reference client (plaintext; reads the dev stub
  attestation the enclave always sends).

There is no `eth_*` proxy crate — reads go straight to the builder.

## Commands

```bash
cargo test -p tog-proto -p tog-core     # the logic that matters (verifiable on any host)
cargo build                             # whole workspace (host targets)
cargo run  -p tog-enclave               # PLAINTEXT dev server on :1546
cargo run  -p tog-client -- --addr 127.0.0.1:1546 demo   # end-to-end smoke test
# (the enclave always sends a stub attestation; the client always reads it — no flags)
cargo clippy && cargo fmt

# Enclave build (Linux SGX host, NIGHTLY — see README "Build-host setup"):
rustup target add x86_64-fortanix-unknown-sgx --toolchain nightly
cargo +nightly build --release -p tog-enclave --target x86_64-fortanix-unknown-sgx
```

## Constraints to remember

- **Apple-Silicon Macs cannot build for SGX or run enclaves** (no HW, no
  simulator). The plaintext dev path + `tog-core` tests work there.
- **Fortanix EDP requires Rust nightly.**
- Enclave config: `TOG_ENCLAVE_BIND` (default `0.0.0.0:1546`) and `TOG_BASE_FEE`
  are read from env on the **host dev build only** — a real EDP enclave does NOT
  inherit the host/container environment, so it uses the built-in defaults. Bake
  any enclave-affecting config into the build, not env.
- The enclave holds no chain state and an ephemeral pool (lost on restart;
  in-enclave time is untrusted).
