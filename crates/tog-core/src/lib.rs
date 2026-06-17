//! The trusted ordering core.
//!
//! This crate is the *only* thing that genuinely needs to live inside the SGX
//! enclave. It replaces reth's `OpTransactionPool` + `CoinbaseTipOrdering` with
//! a self-contained, pure-Rust reimplementation that:
//!
//!   1. decodes an EIP-2718 raw transaction,
//!   2. recovers its signer (RustCrypto `k256`, no C secp256k1),
//!   3. keeps it in an in-enclave pool, and
//!   4. produces a deterministic, tip-priority, nonce-respecting ordering.
//!
//! It has no async runtime, no networking, no file I/O and no C dependencies —
//! all the things that make the full reth stack impossible to compile for
//! `x86_64-fortanix-unknown-sgx`.
//!
//! ## Fidelity caveat
//!
//! [`Mempool::best_transaction_hashes`] approximates reth's pending-pool
//! ordering: a greedy tip-priority merge that always emits the highest-tip
//! *ready* (lowest outstanding nonce) transaction per sender. This matches the
//! intent of `CoinbaseTipOrdering`, but the exact tie-breaking and base-fee
//! handling MUST be validated against the builder you feed before you rely on
//! the "guarantee". See `docs/sgx-refactor.md`.

mod mempool;

pub use mempool::{Mempool, MempoolError, PoolStats};
