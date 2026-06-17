//! Connection transport.
//!
//! Currently **plaintext on every target** — including inside a real SGX
//! enclave. There is no TLS or real attestation yet; the only attestation is the
//! application-level dev stub (`TOG_STUB_ATTEST`, see `main.rs`), which presents
//! a fake document so the attested-channel flow can be exercised. A real attested
//! transport is future work.
//!
//! [`Transport::init`] runs once at startup; [`Transport::accept`] wraps each
//! accepted connection and returns a [`Session`] (`Read + Write`). Keeping this
//! seam means a real transport can be slotted in later without touching `main.rs`.

use std::io;
use std::net::TcpStream;

/// Per-process transport state. Empty while the transport is plaintext.
pub struct Transport;

/// The connection type carrying the framed protocol. Plaintext (`TcpStream`)
/// today; would become a TLS session type when a real transport lands.
pub type Session = TcpStream;

impl Transport {
    pub fn init() -> Result<Self, Box<dyn std::error::Error>> {
        Ok(Transport)
    }

    pub fn accept(&self, stream: TcpStream) -> io::Result<Session> {
        Ok(stream)
    }
}
