# Transaction Order Guarantor — SGX / Fortanix EDP

Runs the **transaction ordering** (the mempool) inside an Intel SGX enclave as
the smallest trusted unit, with all passthrough/communication outside. One input
(`send_raw_transaction`) terminates inside the enclave; two outputs
(`get_raw_transactions`, `get_best_transaction_hashes`) are read from it. The
enclave is attestable via RA-TLS (Fortanix **Enclave Manager**).

## Showstoppers / things you must provide (read first)

1. **No SGX on an Apple-Silicon Mac.** arm64 has no SGX hardware and there is no
   Fortanix software simulator. You can *edit* and run the plaintext dev path,
   but you **cannot build the enclave for SGX or run it** there. Provision a
   **Linux x86_64 host with SGX2** — easiest is an **Azure confidential VM**
   (DCsv3 / DCdsv3, Ice Lake; large EPC), or bare-metal Ice Lake / Sapphire
   Rapids Xeon. Avoid SGX1 (tiny ~93 MB EPC). Note: **building** the enclave
   image needs only the toolchain (no SGX HW); SGX HW is needed to **run** it.
2. **The reth `OpTransactionPool` cannot go in the enclave.** It transitively
   pulls in Tokio/mio + C dependencies (`c-kzg`, secp256k1-C, mdbx, …). The
   `x86_64-fortanix-unknown-sgx` target cannot compile C via the `cc` crate, so
   that whole stack won't link inside an enclave. The minimal tip ordering is
   therefore **reimplemented in pure Rust** in `crates/tog-core`.
3. **Rust nightly is required** by Fortanix EDP (`rustup target add … --toolchain
   nightly`).
4. **You install the SGX stack on the Linux host yourself** (driver, PSW + AESM,
   DCAP libs, Fortanix tools — see [Build-host setup](#build-host-setup-linux-sgx-box)).
5. **Enclave Manager account.** RA-TLS uses Fortanix Enclave Manager for cert
   issuance; you need an EM account, the enclave registered (MRENCLAVE /
   MRSIGNER), and the EM **node agent** running. Without it, the enclave still
   runs in **plaintext dev mode**.
6. **In-enclave time is untrusted and the pool is volatile.** `SystemTime` is a
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

### No-Tokio model

The enclave uses blocking `std::net` + one OS thread per connection. SGX threads
are preallocated (`threads` in `crates/tog-enclave/Cargo.toml`
`[package.metadata.fortanix-sgx]`), so that number caps concurrent connections.
No async runtime is needed or wanted — it shrinks the TCB and avoids mio (which
doesn't target SGX cleanly).

### Ordering & nonce model (no chain state)

The enclave never sees on-chain account state, and doesn't need it to *order*
transactions. [`tog-core::Mempool`](crates/tog-core/src/mempool.rs):

* **per-sender sequencing** — only the contiguous nonce run from an anchor is
  "ready"; anything past a gap is held until it fills,
* **replace-by-fee** — `(sender, nonce)` collisions collapse to the higher tip,
* **cross-sender priority** — ready heads merged greedily by effective tip.

The anchor is the lowest nonce seen, unless an on-chain nonce is supplied via
`Mempool::set_account_nonce` — an untrusted hint that only refines readiness
(final validity is enforced by the builder when it executes the ordering, so a
wrong hint can at worst make the ordering suboptimal, never unsafe). This
replaces the old "fake the account nonce per tx" trick and needs no
state-provider abstraction.

> Note: `set_account_nonce` is currently an **in-enclave API only** — it is not
> yet exposed over the wire protocol, so today the enclave always anchors on the
> lowest nonce it has seen. Wiring a hint channel is deferred: in the
> direct-RA-TLS model the untrusted host never sees decrypted txs (hence not the
> senders), so a future hint would come from the builder's get-best request or
> an enclave-initiated `eth_getTransactionCount` call.

## Build & run

### Pure-Rust core + dev dataplane (works anywhere, incl. an arm64 Mac)

```bash
cargo test  -p tog-proto -p tog-core   # the logic that matters, verifiable locally
cargo run   -p tog-enclave             # PLAINTEXT dev server on :1546 (not attested)
cargo run   -p tog-host                # passthrough proxy on :1545
```

### Test client (`tog-client`)

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

The plaintext `demo` path is verified working end-to-end on an arm64 Mac.

### Enclave (Linux SGX host, nightly)

```bash
rustup target add x86_64-fortanix-unknown-sgx --toolchain nightly
cargo install fortanix-sgx-tools sgxs-tools

# uncomment the [target.'cfg(target_env = "sgx")'.dependencies] block in
#   crates/tog-enclave/Cargo.toml  (em-app + mbedtls)
cargo +nightly build --release -p tog-enclave --target x86_64-fortanix-unknown-sgx

# with the runner configured (see setup §5), this converts + signs + runs:
cargo +nightly run -p tog-enclave --target x86_64-fortanix-unknown-sgx
# …or do it by hand:
ftxsgx-elf2sgxs target/x86_64-fortanix-unknown-sgx/release/tog-enclave \
    --heap-size 2147483648 --stack-size 262144 --threads 16 --debug \
    --output tog-enclave.sgxs
sgxs-sign --key signing-key.pem tog-enclave.sgxs tog-enclave.sig
ftxsgx-runner tog-enclave.sgxs
```

Docker: see [`Dockerfile.enclave`](Dockerfile.enclave) (run with
`--device /dev/sgx_enclave --device /dev/sgx_provision` and the AESM socket
mounted). It cannot be built/run on a Mac.

## Build-host setup (Linux SGX box)

Ubuntu 22.04 x86_64. Building needs only steps 3–5; running/testing needs 0–2 + 6.

**0. Hardware / VM** — CPU with SGX **+ FLC**; for SGX2 / large EPC use Azure
DCsv3 / DCdsv3 (pre-enabled) or Ice Lake / Sapphire Rapids bare metal (enable SGX
in BIOS, not "software controlled"). Kernel ≥ 5.11.

**1. SGX driver** — kernel ≥ 5.11 is in-tree; verify `ls /dev/sgx_enclave
/dev/sgx_provision` and add yourself to the `sgx` group. Older kernels: install
Intel's out-of-tree DCAP driver (`/dev/isgx`).

**2. PSW + AESM** (to run + attest):
```bash
echo "deb https://download.01.org/intel-sgx/sgx_repo/ubuntu $(lsb_release -cs) main" \
  | sudo tee /etc/apt/sources.list.d/intel-sgx.list
curl -sSL https://download.01.org/intel-sgx/sgx_repo/ubuntu/intel-sgx-deb.key | sudo apt-key add -
sudo apt-get update
sudo apt-get install -y sgx-aesm-service libsgx-aesm-launch-plugin \
    libsgx-aesm-quote-ex-plugin libsgx-aesm-ecdsa-plugin \
    libsgx-dcap-ql libsgx-dcap-default-qpl
```
Check `systemctl status aesmd` and `/var/run/aesmd/aesm.socket`.

**3. Build deps** (Fortanix tools + the C deps `mbedtls` pulls in):
```bash
sudo apt-get install -y build-essential cmake pkg-config libssl-dev protobuf-compiler
```

**4. Rust nightly + the EDP target:**
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup toolchain install nightly
rustup target add x86_64-fortanix-unknown-sgx --toolchain nightly
```

**5. Fortanix tools + cargo runner** — `cargo install fortanix-sgx-tools
sgxs-tools`, then add to `~/.cargo/config.toml`:
```toml
[target.x86_64-fortanix-unknown-sgx]
runner = "ftxsgx-runner-cargo"
```

**6. Verify the box:** `sgx-detect` (reports SGX enabled, driver, AESM, and
whether it can actually run an enclave).

## RA-TLS via Enclave Manager — implemented (best-effort, build on Linux)

[`crates/tog-enclave/src/transport.rs`](crates/tog-enclave/src/transport.rs)
contains the real wiring under `cfg(target_env = "sgx")`, matching the documented
`em-app` / Fortanix `mbedtls` APIs:

1. `Transport::init()` (once, at startup): generate an in-enclave RSA key
   (`Pk::generate_rsa(&mut Rdrand, 3072, 0x10001)`), call
   `em_app::get_fortanix_em_certificate(node_agent_url, cn, &mut key)` — EM
   verifies the MRENCLAVE/MRSIGNER quote and returns an issued cert — then build
   a reusable `mbedtls::ssl::Config` (`set_rng` + `push_cert`).
2. `Transport::accept()` (per connection): `Context::establish(stream, None)`.
   TLS terminates inside the enclave; the `Context` is `Read + Write`, so the
   dispatch loop is unchanged.

Clients verify the cert with `tog-client --features ratls` (mbedtls client,
`AuthMode::Required`, CA = Enclave Manager root). Reference:
<https://edp.fortanix.com/docs/> and the `em-app` examples.

**To build it on the Linux SGX box:**

* Uncomment the `[target.'cfg(target_env = "sgx")'.dependencies]` block in
  `crates/tog-enclave/Cargo.toml` (`em-app` via git, `mbedtls`). Let `em-app`
  drive the `mbedtls` version if resolution conflicts.
* Set `NODE_AGENT_URL` (EM node agent) and `TOG_TLS_CN` env vars.
* **Verify one field name:** `transport.rs` reads the issued PEM from
  `issued.certificate_response.certificate`. Confirm that against your
  `em_node_agent_client::models` version — if it's a plain `String` (not
  `Option<String>`), drop the `ok_or`. This is the most likely first compile error.

The SGX path **cannot be compiled or tested on a Mac**. The host build keeps it
`cfg`-gated, so `cargo check -p tog-enclave` stays green everywhere.

## Validate against your builder

`tog-core`'s ordering implements gap-aware per-sender sequencing, replace-by-fee
and greedy tip priority. Still **validate it against your builder's actual
selection** (base-fee handling, tie-breaks) before treating the output as a
guarantee. OP deposit txs (type `0x7E`) are not expected via
`send_raw_transaction` and are not handled.
