//! Connection transport.
//!
//! Two implementations selected by target:
//!
//! * **host (`not(target_env = "sgx")`)** — plaintext TCP so the whole
//!   dataplane is exercisable in dev without SGX hardware.
//! * **SGX (`target_env = "sgx")`** — RA-TLS via Fortanix **Enclave Manager**.
//!   A key is generated *inside* the enclave; `em_app` attests it and gets an
//!   EM-issued X.509 cert bound to that key; the connection's TLS handshake
//!   terminates inside the enclave, so the untrusted runner/host only ever sees
//!   ciphertext and cannot drop, reorder, read or inject transactions.
//!
//! Both expose the same surface — [`Transport::init`] (once, at startup) and
//! [`Transport::accept`] (per connection) returning a [`Session`] that is
//! `Read + Write`. `main.rs` is identical across both.
//!
//! ## Status
//! The SGX path is *best-effort, uncompiled* code: it matches the documented
//! `em-app` / Fortanix `mbedtls` APIs (see Cargo.toml for the deps to enable),
//! but it can only be built on a Linux SGX host. The one field most likely to
//! need adjusting per em-app version is flagged inline (`certificate`).

use std::net::TcpStream;

#[cfg(not(target_env = "sgx"))]
mod imp {
    use super::TcpStream;
    use std::io;

    /// Dev transport — NOT confidential, NOT attested.
    pub struct Transport;

    /// Plaintext stream in dev.
    pub type Session = TcpStream;

    impl Transport {
        pub fn init() -> Result<Self, Box<dyn std::error::Error>> {
            Ok(Transport)
        }

        pub fn accept(&self, stream: TcpStream) -> io::Result<Session> {
            Ok(stream)
        }
    }
}

#[cfg(target_env = "sgx")]
mod imp {
    use super::TcpStream;
    use std::io;
    use std::sync::Arc;

    use em_app::get_fortanix_em_certificate;
    use mbedtls::pk::Pk;
    use mbedtls::rng::{CtrDrbg, Rdrand};
    use mbedtls::ssl::config::{Endpoint, Preset, Transport as MbedTransport};
    use mbedtls::ssl::{Config, Context};
    use mbedtls::x509::Certificate;

    /// TLS session whose handshake terminates inside the enclave.
    pub type Session = Context<TcpStream>;

    /// Holds the reusable server config (built once; cert + key live here).
    pub struct Transport {
        config: Arc<Config>,
    }

    impl Transport {
        pub fn init() -> Result<Self, Box<dyn std::error::Error>> {
            // The node agent endpoint is provided by the EM runtime; the CN is
            // whatever identity you registered the enclave under.
            let node_agent_url =
                std::env::var("NODE_AGENT_URL").unwrap_or_else(|_| "http://localhost:9092".into());
            let common_name =
                std::env::var("TOG_TLS_CN").unwrap_or_else(|_| "tog-enclave".into());

            // 1. Generate the key INSIDE the enclave (RDRAND-seeded). The private
            //    key never leaves the enclave.
            let mut rng = Rdrand;
            let mut key = Pk::generate_rsa(&mut rng, 3072, 0x10001)?;

            // 2. Attest + obtain an EM-issued certificate bound to `key`.
            //    Enclave Manager verifies our MRENCLAVE/MRSIGNER quote before it
            //    signs. `Pk` implements `CsrSigner` via em-app's blanket impl,
            //    so `&mut key` is the signer.
            let issued = get_fortanix_em_certificate(&node_agent_url, &common_name, &mut key)
                .map_err(|e| format!("enclave manager certificate request failed: {e}"))?;

            // 3. Pull the issued leaf certificate (PEM). NOTE: confirm this field
            //    name against your `em_node_agent_client::models` version — it is
            //    the PEM of the issued application certificate. If the field is a
            //    plain `String` rather than `Option<String>`, drop the `ok_or`.
            let leaf_pem = issued
                .certificate_response
                .certificate
                .ok_or("Enclave Manager returned no certificate")?;

            // mbedtls' PEM parser requires a NUL-terminated buffer.
            let mut pem = leaf_pem.into_bytes();
            if pem.last() != Some(&0) {
                pem.push(0);
            }
            let cert_chain = Arc::new(Certificate::from_pem_multiple(&pem)?);
            let key = Arc::new(key);

            // 4. Build the reusable server config.
            let rng = Arc::new(CtrDrbg::new(Arc::new(Rdrand), None)?);
            let mut config =
                Config::new(Endpoint::Server, MbedTransport::Stream, Preset::Default);
            config.set_rng(rng);
            config.push_cert(cert_chain, key)?;

            Ok(Transport { config: Arc::new(config) })
        }

        pub fn accept(&self, stream: TcpStream) -> io::Result<Session> {
            // TLS terminates INSIDE the enclave.
            let mut ctx = Context::new(self.config.clone());
            ctx.establish(stream, None)
                .map_err(|e| io::Error::other(format!("tls handshake failed: {e}")))?;
            Ok(ctx)
        }
    }
}

pub use imp::Transport;
// `Session` is the concrete `Read + Write` type per target; re-exported for
// callers that want to name it (main.rs uses inference, hence the allow).
#[allow(unused_imports)]
pub use imp::Session;
