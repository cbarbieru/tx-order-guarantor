//! Wire protocol spoken across the enclave boundary.
//!
//! The enclave terminates an RA-TLS connection and then speaks this framed
//! request/response protocol on top of the (now confidential, integrity-
//! protected) byte stream. There are exactly three operations, mirroring the
//! original JSON-RPC surface:
//!
//!   * [`Request::SendRawTransaction`]      (input  — stream terminates in enclave)
//!   * [`Request::GetRawTransactions`]      (output — drains the raw buffer)
//!   * [`Request::GetBestTransactionHashes`](output — the computed ordering)
//!
//! Framing is deliberately trivial — a 4-byte big-endian length prefix followed
//! by a JSON body — so it has zero non-Rust dependencies and is easy to drive
//! from any client. We reuse hex strings (`0x...`) for raw txs and hashes so the
//! payloads look identical to the values the legacy JSON-RPC server exchanged.

use std::io::{self, Read, Write};

use serde::{Serialize, de::DeserializeOwned};

/// Matches the legacy server's `max_request_body_size` / `max_response_body_size`.
pub const MAX_FRAME_BYTES: usize = 10 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(tag = "method", content = "params", rename_all = "camelCase")]
pub enum Request {
    /// `eth_sendRawTransaction` — a single `0x`-prefixed RLP/2718-encoded tx.
    SendRawTransaction(String),
    /// `tog_getRawTransactions` — drain and return the raw tx buffer.
    GetRawTransactions,
    /// `tog_getBestTransactionHashes` — return the enclave-computed ordering.
    GetBestTransactionHashes,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(tag = "result", content = "data", rename_all = "camelCase")]
pub enum Response {
    /// Hash assigned to an accepted transaction (`0x`-prefixed, 32 bytes).
    Hash(String),
    /// Raw transactions as `0x`-prefixed hex, in insertion order.
    RawTransactions(Vec<String>),
    /// Ordered transaction hashes (`0x`-prefixed), best-first.
    BestTransactionHashes(Vec<String>),
    /// A handled error (bad hex, decode/recovery failure, etc.).
    Error(String),
}

#[derive(Debug, thiserror::Error)]
pub enum ProtoError {
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("frame too large: {0} bytes (max {MAX_FRAME_BYTES})")]
    FrameTooLarge(usize),
}

/// Write a length-prefixed JSON frame. Works over any `Write` — a TLS stream in
/// the enclave, a plain `TcpStream` in dev.
pub fn write_frame<W: Write, T: Serialize>(w: &mut W, msg: &T) -> Result<(), ProtoError> {
    let body = serde_json::to_vec(msg)?;
    if body.len() > MAX_FRAME_BYTES {
        return Err(ProtoError::FrameTooLarge(body.len()));
    }
    w.write_all(&(body.len() as u32).to_be_bytes())?;
    w.write_all(&body)?;
    w.flush()?;
    Ok(())
}

/// Read a single length-prefixed JSON frame.
pub fn read_frame<R: Read, T: DeserializeOwned>(r: &mut R) -> Result<T, ProtoError> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(ProtoError::FrameTooLarge(len));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    Ok(serde_json::from_slice(&body)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_over_a_pipe() {
        let mut buf = Vec::new();
        let req = Request::SendRawTransaction("0xdeadbeef".into());
        write_frame(&mut buf, &req).unwrap();

        let mut cursor = io::Cursor::new(buf);
        let got: Request = read_frame(&mut cursor).unwrap();
        assert_eq!(req, got);
    }

    #[test]
    fn response_variants_serialize_distinctly() {
        for resp in [
            Response::Hash("0x01".into()),
            Response::RawTransactions(vec!["0x02".into()]),
            Response::BestTransactionHashes(vec!["0x03".into()]),
            Response::Error("nope".into()),
        ] {
            let mut buf = Vec::new();
            write_frame(&mut buf, &resp).unwrap();
            let mut cursor = io::Cursor::new(buf);
            let got: Response = read_frame(&mut cursor).unwrap();
            assert_eq!(resp, got);
        }
    }
}
