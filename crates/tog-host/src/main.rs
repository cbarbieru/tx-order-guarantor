//! Untrusted passthrough proxy.
//!
//! Forwards read-only `eth_*` calls to the builder. The order-sensitive methods
//! (`eth_sendRawTransaction`, `tog_getRawTransactions`,
//! `tog_getBestTransactionHashes`) are NOT served here — clients and the builder
//! reach those directly on the enclave's endpoint.

use std::env;
use std::net::SocketAddr;
use std::sync::Arc;

use jsonrpsee::core::client::ClientT;
use jsonrpsee::http_client::{HttpClient, HttpClientBuilder};
use jsonrpsee::server::{ServerBuilder, ServerConfig};
use jsonrpsee::types::{ErrorObject, ErrorObjectOwned};
use jsonrpsee::RpcModule;
use serde_json::Value;

/// Read-only methods proxied straight through to the builder.
const PASSTHROUGH: &[&str] = &[
    "eth_chainId",
    "eth_getTransactionCount",
    "eth_feeHistory",
    "eth_estimateGas",
    "eth_blockNumber",
    "eth_getBlockByNumber",
    "eth_getTransactionReceipt",
    "eth_getBalance",
    "eth_gasPrice",
    "eth_blobBaseFee",
    "eth_getAccountInfo",
    "eth_getCode",
    "eth_getBlockReceipts",
];

#[tokio::main]
async fn main() -> anyhow_lite::Result<()> {
    let host = env::var("BUILDER_HOST").unwrap_or_else(|_| "op-rbuilder".to_string());
    let port = env::var("BUILDER_PORT").unwrap_or_else(|_| "8545".to_string());
    let url = format!("http://{host}:{port}");
    println!("builder url = {url}");

    let builder_client: Arc<HttpClient> = Arc::new(HttpClientBuilder::default().build(&url)?);

    let mut module = RpcModule::new(builder_client);
    for &method in PASSTHROUGH {
        let name = Arc::new(method.to_string());
        module.register_async_method(method, move |params, client, _| {
            let name = name.clone();
            async move {
                let params_vec: Vec<Value> = params.parse().unwrap_or_default();
                client
                    .request::<Value, Vec<Value>>(&name, params_vec)
                    .await
                    .map_err(|e| ErrorObjectOwned::from(ErrorObject::owned(-32000, e.to_string(), None::<()>)))
            }
        })?;
    }

    let addr: SocketAddr = env::var("TOG_HOST_BIND")
        .unwrap_or_else(|_| "0.0.0.0:1545".to_string())
        .parse()?;
    let config = ServerConfig::builder()
        .max_connections(1_000)
        .max_request_body_size(10 * 1024 * 1024)
        .max_response_body_size(10 * 1024 * 1024)
        .build();
    let server = ServerBuilder::with_config(config).build(addr).await?;
    let handle = server.start(module);
    println!("🚀 passthrough proxy listening on http://{addr}");
    handle.stopped().await;
    Ok(())
}

/// Minimal local error glue so we don't add an `anyhow` dependency just for
/// `main`'s `?`.
mod anyhow_lite {
    pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
}
