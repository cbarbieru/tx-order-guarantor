//! Client for the enclave's framed protocol.
//!
//! [`Client`] is generic over any `Read + Write` connection, so the same
//! request/response logic drives both the plaintext dev transport and the
//! RA-TLS transport. Connectors:
//!
//!   * [`connect_plain`] — plain TCP (dev enclave).
//!   * [`connect_ratls`] — (feature `ratls`) mbedtls client that verifies the
//!     enclave's Enclave-Manager-issued certificate against EM's CA root.

use std::fmt;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use tog_proto::{read_frame, write_frame, Request, Response};

/// A connected client. Each method is one request/response round trip; the
/// connection stays open across calls.
pub struct Client<S> {
    conn: S,
}

impl<S: Read + Write> Client<S> {
    pub fn new(conn: S) -> Self {
        Self { conn }
    }

    /// `eth_sendRawTransaction` — returns the assigned tx hash (`0x…`).
    pub fn send_raw_transaction(&mut self, raw_hex: &str) -> Result<String, ClientError> {
        match self.call(Request::SendRawTransaction(raw_hex.to_string()))? {
            Response::Hash(h) => Ok(h),
            Response::Error(e) => Err(ClientError::Server(e)),
            other => Err(ClientError::Unexpected(format!("{other:?}"))),
        }
    }

    /// `tog_getRawTransactions` — drains and returns the raw tx buffer.
    pub fn get_raw_transactions(&mut self) -> Result<Vec<String>, ClientError> {
        match self.call(Request::GetRawTransactions)? {
            Response::RawTransactions(v) => Ok(v),
            Response::Error(e) => Err(ClientError::Server(e)),
            other => Err(ClientError::Unexpected(format!("{other:?}"))),
        }
    }

    /// `tog_getBestTransactionHashes` — the enclave-computed ordering.
    pub fn get_best_transaction_hashes(&mut self) -> Result<Vec<String>, ClientError> {
        match self.call(Request::GetBestTransactionHashes)? {
            Response::BestTransactionHashes(v) => Ok(v),
            Response::Error(e) => Err(ClientError::Server(e)),
            other => Err(ClientError::Unexpected(format!("{other:?}"))),
        }
    }

    fn call(&mut self, req: Request) -> Result<Response, ClientError> {
        write_frame(&mut self.conn, &req)?;
        Ok(read_frame(&mut self.conn)?)
    }
}

/// Connect over plain TCP, retrying briefly so a just-started dev enclave is
/// caught without manual timing.
pub fn connect_plain(addr: &str) -> std::io::Result<Client<TcpStream>> {
    let mut last = None;
    for _ in 0..50 {
        match TcpStream::connect(addr) {
            Ok(s) => return Ok(Client::new(s)),
            Err(e) => {
                last = Some(e);
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
    Err(last.expect("at least one attempt"))
}

#[cfg(feature = "ratls")]
pub use ratls::connect_ratls;

#[cfg(feature = "ratls")]
mod ratls {
    use super::*;
    use std::sync::Arc;

    use mbedtls::rng::{CtrDrbg, OsEntropy};
    use mbedtls::ssl::config::{Endpoint, Preset, Transport as MbedTransport};
    use mbedtls::ssl::{Config, Context};
    use mbedtls::x509::Certificate;

    /// Connect to the enclave's RA-TLS endpoint and verify its
    /// Enclave-Manager-issued certificate against the EM CA root in
    /// `ca_pem_path`. `server_name`, if given, is checked against the cert.
    ///
    /// Because EM only issues the cert after verifying the enclave's
    /// MRENCLAVE/MRSIGNER quote, a successful handshake against EM's CA is proof
    /// you're talking to that attested enclave build.
    pub fn connect_ratls(
        addr: &str,
        ca_pem_path: &str,
        server_name: Option<&str>,
    ) -> Result<Client<Context<TcpStream>>, ClientError> {
        let ca = std::fs::read(ca_pem_path)?;
        // mbedtls' PEM parser wants a NUL-terminated buffer.
        let mut ca_pem = ca;
        if ca_pem.last() != Some(&0) {
            ca_pem.push(0);
        }
        let ca = Arc::new(Certificate::from_pem_multiple(&ca_pem).map_err(tls_err)?);

        let entropy = Arc::new(OsEntropy::new());
        let rng = Arc::new(CtrDrbg::new(entropy, None).map_err(tls_err)?);

        let mut config = Config::new(Endpoint::Client, MbedTransport::Stream, Preset::Default);
        config.set_rng(rng);
        // Preset::Default sets AuthMode::Required for clients, so the CA list is
        // enforced — an unverifiable cert aborts the handshake.
        config.set_ca_list(ca, None);

        let stream = TcpStream::connect(addr)?;
        let mut ctx = Context::new(Arc::new(config));
        ctx.establish(stream, server_name).map_err(tls_err)?;
        Ok(Client::new(ctx))
    }

    fn tls_err(e: mbedtls::Error) -> ClientError {
        ClientError::Tls(e.to_string())
    }
}

/// Build a signed EIP-1559 transaction as the `0x`-prefixed 2718 bytes a client
/// would submit. `key_byte` selects a deterministic signer (distinct senders),
/// `priority_fee` drives the tip ordering.
pub fn sample_raw_tx(key_byte: u8, nonce: u64, priority_fee: u128) -> String {
    use alloy_consensus::{SignableTransaction, TxEip1559, TxEnvelope};
    use alloy_eips::eip2718::Encodable2718;
    use alloy_primitives::{Address, B256, TxKind, U256};
    use alloy_signer::SignerSync;
    use alloy_signer_local::PrivateKeySigner;

    let signer = PrivateKeySigner::from_bytes(&B256::repeat_byte(key_byte)).expect("valid key");
    let tx = TxEip1559 {
        chain_id: 10,
        nonce,
        gas_limit: 21_000,
        max_fee_per_gas: priority_fee + 1_000_000_000,
        max_priority_fee_per_gas: priority_fee,
        to: TxKind::Call(Address::repeat_byte(0xab)),
        value: U256::ZERO,
        access_list: Default::default(),
        input: Default::default(),
    };
    let sig = signer.sign_hash_sync(&tx.signature_hash()).expect("sign");
    let env: TxEnvelope = tx.into_signed(sig).into();
    let bytes = env.encoded_2718();
    format!("0x{}", hex::encode(bytes))
}

#[derive(Debug)]
pub enum ClientError {
    Io(std::io::Error),
    Proto(tog_proto::ProtoError),
    /// The enclave returned a handled error (bad hex, decode/recovery failure).
    Server(String),
    /// TLS/handshake/verification failure (feature `ratls`).
    Tls(String),
    Unexpected(String),
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClientError::Io(e) => write!(f, "io: {e}"),
            ClientError::Proto(e) => write!(f, "protocol: {e}"),
            ClientError::Server(e) => write!(f, "enclave error: {e}"),
            ClientError::Tls(e) => write!(f, "tls: {e}"),
            ClientError::Unexpected(e) => write!(f, "unexpected response: {e}"),
        }
    }
}

impl std::error::Error for ClientError {}

impl From<std::io::Error> for ClientError {
    fn from(e: std::io::Error) -> Self {
        ClientError::Io(e)
    }
}

impl From<tog_proto::ProtoError> for ClientError {
    fn from(e: tog_proto::ProtoError) -> Self {
        ClientError::Proto(e)
    }
}
