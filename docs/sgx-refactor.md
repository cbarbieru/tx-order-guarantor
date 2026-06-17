# SGX refactor — running the mempool inside a Fortanix EDP enclave

## Goal

Run the **transaction ordering** (the mempool) inside an Intel SGX enclave as
the smallest trusted unit, with all passthrough/communication outside. One input
(`send_raw_transaction`) terminates inside the enclave; two outputs
(`get_raw_transactions`, `get_best_transaction_hashes`) are read from it. The
enclave is attestable via RA-TLS (Fortanix **Enclave Manager**).

## Showstoppers / things you must provide (read first)

1. **No SGX on the dev Mac.** This repo's dev machine is Apple Silicon (arm64);
   there is no SGX hardware and no Fortanix software simulator. You can *edit*
   here, but you **cannot build the enclave for SGX or run it**. Provision a
   **Linux x86_64 host with SGX2** — easiest is an **Azure confidential VM**
   (DCsv3 / DCdsv3, Ice Lake; large EPC), or bare-metal Ice Lake / Sapphire
   Rapids Xeon. Avoid SGX1 (tiny ~93 MB EPC).
2. **The reth `OpTransactionPool` cannot go in the enclave.** It transitively
   pulls in Tokio/mio + C dependencies (`c-kzg`, secp256k1-C, mdbx, …). The
   `x86_64-fortanix-unknown-sgx` target cannot compile C via the `cc` crate, so
   that whole stack won't link inside an enclave. We therefore **reimplement the
   minimal tip ordering in pure Rust** in `crates/tog-core`. The legacy reth
   pool stays out of the enclave.
3. **You must install the SGX stack on the Linux host yourself** (driver, PSW +
   AESM, DCAP libs, Fortanix tools — see below).
4. **Enclave Manager account.** RA-TLS here uses Fortanix Enclave Manager for
   cert issuance; you need an EM account + the enclave registered (MRENCLAVE /
   MRSIGNER). Without it, the enclave still runs in **plaintext dev mode**.
5. **In-enclave time is untrusted and the pool is volatile.** `SystemTime` is a
   host usercall (host can lie); enclave memory is wiped on restart (no sealing
   yet). Fine for an ephemeral mempool — just know it.

## Architecture

```
                    ┌────────────────────── untrusted host ──────────────────────┐
   eth_* (RO) ─────►│  tog-host : jsonrpsee proxy ──► builder (op-rbuilder)        │
                    └─────────────────────────────────────────────────────────────┘

   submitters ─── RA-TLS ───►┐
   builder    ─── RA-TLS ───►│  tog-enclave (SGX)  : terminates TLS *inside*,
                             │   ├─ send_raw_transaction   (input)
                             │   ├─ get_raw_transactions   (output, drains buffer)
                             └─► └─ get_best_transaction_hashes (output, ordering)
                                  state: tog-core::Mempool (in enclave memory)
```

* **Direct RA-TLS to the enclave** (the chosen trust model): the untrusted
  runner/host only sees ciphertext, so it cannot drop, reorder, read or inject
  transactions. That is what makes the ordering "guarantee" meaningful. If you
  ever relay ingestion through the host instead, the host re-enters the
  integrity path and the guarantee weakens to "the host behaved".

### Crates

| crate         | target                         | role |
|---------------|--------------------------------|------|
| `tog-proto`   | any (pure)                     | length-prefixed JSON wire protocol (3 ops) |
| `tog-core`    | any (pure)                     | **TCB**: decode + signer recovery + tip ordering, unit-tested |
| `tog-enclave` | `x86_64-fortanix-unknown-sgx`  | RA-TLS listener (no Tokio), dispatches to `tog-core` |
| `tog-host`    | normal                         | untrusted `eth_*` passthrough proxy |
| `tog-client`  | normal                         | test/reference client + RA-TLS verifier (`ratls` feature) |

The root `tx-order-guarantor` package is the **legacy monolith**, left intact
and runnable. Slim it down (or retire it) once the split is validated.

## What runs where, and the no-Tokio model

The enclave uses blocking `std::net` + one OS thread per connection. SGX threads
are preallocated (`threads` in `crates/tog-enclave/Cargo.toml`
`[package.metadata.fortanix-sgx]`), so that number caps concurrent connections.
No async runtime is needed or wanted — it shrinks the TCB and avoids mio (which
doesn't target SGX cleanly).

## Build & run

### Pure-Rust core (works anywhere, including the Mac)

```bash
cargo test -p tog-proto -p tog-core    # the logic that matters, verifiable locally
cargo run  -p tog-enclave              # PLAINTEXT dev server on :1546 (not attested)
cargo run  -p tog-host                 # passthrough proxy on :1545
```

### Test client (`tog-client`)

Drives the three ops over the wire protocol. Plaintext by default; RA-TLS behind
the `ratls` feature.

```bash
# end-to-end smoke test against the dev enclave (start it first, on :1546):
cargo run -p tog-client -- --addr 127.0.0.1:1546 demo
#   → sends sample txs with tips 5/100/50; get-best returns 100,50,5; get-raw drains.

cargo run -p tog-client -- send 0x02f8...      # submit a real raw tx
cargo run -p tog-client -- get-best            # print the ordering

# Against the real enclave, verifying its EM-issued cert (host-side, builds
# mbedtls — needs cmake + a C compiler, but NOT SGX, so it compiles anywhere):
cargo run -p tog-client --features ratls -- \
    --ratls --ca em-ca.pem --server-name tog-enclave --addr ENCLAVE_HOST:1546 demo
```

The `connect_ratls` verifier (mbedtls client, `AuthMode::Required`, CA =
Enclave Manager root) is best-effort like the enclave's SGX path, but unlike it
you *can* compile-check it locally with `--features ratls`.

The plaintext `demo` path is verified working end-to-end on the dev Mac.

### Enclave (Linux SGX host only)

```bash
rustup target add x86_64-fortanix-unknown-sgx
cargo install fortanix-sgx-tools sgxs-tools
cargo build --release -p tog-enclave --target x86_64-fortanix-unknown-sgx
# convert + sign + run:
ftxsgx-elf2sgxs target/x86_64-fortanix-unknown-sgx/release/tog-enclave \
    --heap-size 2147483648 --stack-size 262144 --threads 16 --debug \
    --output tog-enclave.sgxs
sgxs-sign --key signing-key.pem tog-enclave.sgxs tog-enclave.sig
ftxsgx-runner tog-enclave.sgxs
```

Docker: see [`Dockerfile.enclave`](../Dockerfile.enclave) (run with
`--device /dev/sgx_enclave --device /dev/sgx_provision` and the AESM socket
mounted).

### Host SGX stack to install (Ubuntu 22.04)

Intel SGX apt repo → `libsgx-enclave-common libsgx-urts libsgx-quote-ex
libsgx-dcap-ql libsgx-dcap-default-qpl sgx-aesm-service`. Kernel ≥ 5.11 gives
`/dev/sgx_enclave` + `/dev/sgx_provision`. For DCAP quote verification you also
need a PCCS (caching service) or Intel PCS access configured in the QPL.

## RA-TLS via Enclave Manager — implemented (best-effort, build on Linux)

[`crates/tog-enclave/src/transport.rs`](../crates/tog-enclave/src/transport.rs)
now contains the real wiring under `cfg(target_env = "sgx")`, matching the
documented `em-app` / Fortanix `mbedtls` APIs:

1. `Transport::init()` (once, at startup): generate an in-enclave RSA key
   (`Pk::generate_rsa(&mut Rdrand, 3072, 0x10001)`), call
   `em_app::get_fortanix_em_certificate(node_agent_url, cn, &mut key)` — EM
   verifies our MRENCLAVE/MRSIGNER quote and returns an issued cert — then build
   a reusable `mbedtls::ssl::Config` (`set_rng` + `push_cert`).
2. `Transport::accept()` (per connection): `Context::establish(stream, None)`.
   TLS terminates inside the enclave; the `Context` is `Read + Write`, so the
   dispatch loop is unchanged.

Clients verify the cert against EM's published enclave identity. Reference:
<https://edp.fortanix.com/docs/> and the `em-app` examples
(`fortanix/rust-sgx/em-app/examples`).

**To build it on your Linux SGX box:**

* Uncomment the `[target.'cfg(target_env = "sgx")'.dependencies]` block in
  `crates/tog-enclave/Cargo.toml` (`em-app` via git, `mbedtls`). Let `em-app`
  drive the `mbedtls` version if resolution conflicts.
* Set `NODE_AGENT_URL` (EM node agent) and `TOG_TLS_CN` env vars.
* **Verify one field name:** `transport.rs` reads the issued PEM from
  `issued.certificate_response.certificate`. Confirm that against your
  `em_node_agent_client::models::IssueCertificateResponse` version — if it's a
  plain `String` (not `Option<String>`), drop the `ok_or`. This is the most
  likely first compile error.

This SGX path **cannot be compiled or tested on the dev Mac** (no SGX, and
`em-app` attestation is SGX-only). The host build keeps it `cfg`-gated, so
`cargo check -p tog-enclave` stays green here regardless.

## Fidelity TODO (important)

`tog-core` approximates reth's `CoinbaseTipOrdering`: a greedy tip-priority merge
that always emits the highest effective-tip *ready* (lowest outstanding nonce)
tx per sender, with first-seen tie-breaks. **Validate this against your builder's
actual selection** (base-fee handling, tie-breaks, OP deposit-tx exclusion)
before treating the output as a guarantee. OP deposit txs (type `0x7E`) are not
expected via `send_raw_transaction` and are not handled.
```
