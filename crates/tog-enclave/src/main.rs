//! The SGX enclave: the smallest trusted unit.
//!
//! Responsibilities (and nothing more):
//!   * accept client connections (see [`transport`] — plaintext for now),
//!   * speak the [`tog_proto`] framed protocol,
//!   * keep the ordered mempool ([`tog_core::Mempool`]) in enclave memory.
//!
//! No Tokio, no async, no file/network I/O beyond the listener — just blocking
//! `std::net` + one thread per connection (bounded by the `threads` count in
//! Cargo.toml's `[package.metadata.fortanix-sgx]`).

mod transport;

use std::env;
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;

use alloy_primitives::{Bytes, B256};
use tog_core::Mempool;
use tog_proto::{read_frame, write_frame, Request, Response, StubAttestation};

type SharedPool = Arc<Mutex<Mempool>>;

fn main() {
    let bind = env::var("TOG_ENCLAVE_BIND").unwrap_or_else(|_| "0.0.0.0:1546".to_string());
    // Base fee used for tip ordering; feed the builder's pending base fee here.
    let base_fee: u64 = env::var("TOG_BASE_FEE").ok().and_then(|v| v.parse().ok()).unwrap_or(0);

    let pool: SharedPool = Arc::new(Mutex::new(Mempool::new(base_fee, 7)));

    // Build the transport once. Plaintext today (no-op); the seam is here so a
    // real attested transport can be added later without touching this loop.
    let transport = Arc::new(
        transport::Transport::init().unwrap_or_else(|e| panic!("transport init failed: {e}")),
    );

    let listener = TcpListener::bind(&bind).unwrap_or_else(|e| panic!("bind {bind}: {e}"));

    if cfg!(not(target_env = "sgx")) {
        eprintln!("⚠️  tog-enclave running OUTSIDE SGX (plaintext, NOT attested) — dev only");
    }
    eprintln!("⚠️  presenting a STUB attestation to every client (fake, NOT real) — dev only");
    println!("🔒 tog-enclave listening on {bind} (base_fee={base_fee})");

    for incoming in listener.incoming() {
        match incoming {
            Ok(stream) => {
                let pool = Arc::clone(&pool);
                let transport = Arc::clone(&transport);
                // One thread per connection. On SGX this consumes a TCS, so the
                // `threads` metadata bounds concurrency.
                thread::spawn(move || {
                    if let Err(e) = serve(&transport, stream, pool) {
                        eprintln!("connection ended: {e}");
                    }
                });
            }
            Err(e) => eprintln!("accept error: {e}"),
        }
    }
}

fn serve(
    transport: &transport::Transport,
    stream: TcpStream,
    pool: SharedPool,
) -> std::io::Result<()> {
    let mut session = transport.accept(stream)?;

    // Always announce a fake attestation before the request loop — stub is the
    // only mode, and the client always reads it (no env gate: EDP enclaves don't
    // inherit the host environment, so this must be baked in, not toggled at
    // runtime). Obvious placeholder measurements + a `stub` tripwire so it can
    // never be confused with a real quote.
    let att = StubAttestation {
        stub: true,
        mr_enclave: format!("0x{}", "de".repeat(32)),
        mr_signer: format!("0x{}", "be".repeat(32)),
        note: "DEV STUB — NOT A REAL SGX QUOTE; proves nothing".to_string(),
    };
    if write_frame(&mut session, &att).is_err() {
        return Ok(());
    }

    loop {
        // A clean EOF / closed connection ends the loop without noise.
        let req: Request = match read_frame(&mut session) {
            Ok(req) => req,
            Err(_) => return Ok(()),
        };
        let resp = dispatch(req, &pool);
        if write_frame(&mut session, &resp).is_err() {
            return Ok(());
        }
    }
}

fn dispatch(req: Request, pool: &SharedPool) -> Response {
    match req {
        Request::SendRawTransaction(hex_str) => {
            let bytes = match decode_hex(&hex_str) {
                Ok(b) => b,
                Err(e) => return Response::Error(e),
            };
            let mut pool = pool.lock().expect("pool mutex poisoned");
            match pool.add_raw_transaction(bytes) {
                Ok(hash) => Response::Hash(hex0x(hash.as_slice())),
                Err(e) => Response::Error(e.to_string()),
            }
        }
        Request::GetRawTransactions => {
            let raws = pool.lock().expect("pool mutex poisoned").drain_raw_transactions();
            Response::RawTransactions(raws.iter().map(|b| hex0x(b.as_ref())).collect())
        }
        Request::GetBestTransactionHashes => {
            let hashes: Vec<B256> =
                pool.lock().expect("pool mutex poisoned").best_transaction_hashes();
            Response::BestTransactionHashes(hashes.iter().map(|h| hex0x(h.as_slice())).collect())
        }
    }
}

fn decode_hex(s: &str) -> Result<Bytes, String> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    hex::decode(s).map(Bytes::from).map_err(|e| format!("invalid hex: {e}"))
}

fn hex0x(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(2 + bytes.len() * 2);
    s.push_str("0x");
    s.push_str(&hex::encode(bytes));
    s
}
