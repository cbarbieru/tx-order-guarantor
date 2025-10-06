mod rpc;
mod noop;

use std::{fs, net::SocketAddr, sync::Arc};
use alloy_primitives::{Bytes, hex};
use anyhow::Ok;
use jsonrpsee::server::ServerBuilder;
use jsonrpsee::http_client::HttpClientBuilder;
use jsonrpsee::core::client::ClientT;
use jsonrpsee::types::{ErrorObject, ErrorObjectOwned};
use jsonrpsee::core::middleware::{Request, Batch, Notification, RpcServiceT};
use jsonrpsee::RpcModule;
use reth_optimism_chainspec::OpChainSpec;
use reth_optimism_primitives::OpPrimitives;
use reth_optimism_txpool::{OpPooledTransaction, OpTransactionPool, OpTransactionValidator};
use reth_tasks::TokioTaskExecutor;
use reth_transaction_pool::{
    blobstore::NoopBlobStore, CoinbaseTipOrdering, PoolConfig, TransactionValidationTaskExecutor,
};
use alloy_genesis::Genesis;
use serde_json::Value;

use crate::noop::NoopProviderTog;
use crate::rpc::GuarantorTxGet;
use crate::rpc::GuarantorApi;

fn load_chain_spec_from_file(path: &str) -> anyhow::Result<OpChainSpec> {
    let genesis_json_string = fs::read_to_string(path)?;
    let genesis: Genesis = serde_json::from_str(&genesis_json_string)?;
    let chain_spec: OpChainSpec = OpChainSpec::from_genesis(genesis);
    Ok(chain_spec)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {

    let chain_spec: OpChainSpec = load_chain_spec_from_file("res/l2-genesis.json")?;
    
    let provider= NoopProviderTog::<OpChainSpec, OpPrimitives>::new(Arc::new(chain_spec));

    let blob_store = NoopBlobStore::default();

    let op_tx_validator = TransactionValidationTaskExecutor::eth_builder(provider.clone())
        .build_with_tasks::<OpPooledTransaction, _, _>(TokioTaskExecutor::default().clone(), blob_store.clone())
        .map(|validator| {
            OpTransactionValidator::new(validator)
        });
    
    let ordering = CoinbaseTipOrdering::<OpPooledTransaction>::default();
    
    let config = PoolConfig::default();

    let pool: OpTransactionPool<_, _, _> =
        OpTransactionPool::new(op_tx_validator, ordering, blob_store.clone(), config);
    
    let builder_client = HttpClientBuilder::default().build("http://localhost:2222").unwrap();

    let api = Arc::new(GuarantorApi::new(pool, provider, builder_client));
    // TODO: generate key pair and 1) expose pk for verification 2) change 'tog_getBestTransactionHashes' to return signed has order 

    let mut mod_tx_send = RpcModule::new(api.clone());
    mod_tx_send
        .register_async_method("eth_sendRawTransaction", |params, api, _| async move {
            let (raw_tx,): (String,) = params.parse().map_err(|err| {
                ErrorObjectOwned::owned(-32602, "Invalid params", Some(err.to_string()))
            })?;
            let raw_tx_str = raw_tx.strip_prefix("0x").unwrap_or(&raw_tx);
            let decoded = hex::decode(raw_tx_str).map_err(|err| {
                ErrorObjectOwned::owned(-32602, "Invalid hex", Some(err.to_string()))
            })?;
            let bytes = Bytes::from(decoded);
            api.send_raw_transaction(bytes).await
        })?;
    
    let mut mod_txs_raw = RpcModule::new(api.clone());
    mod_txs_raw
        .register_async_method("tog_getRawTransactions", |_, api, _| async move {
            api.get_raw_transactions().await
        })?;

    let mut mod_tx_get = RpcModule::new(api.clone());
    mod_tx_get
        .register_async_method("tog_getBestTransactionHashes", |_, api, _| async move {
            api.get_best_transaction_hashes().await
        })?;

    let methods = [
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
    ];
    let mut passthrough_module = RpcModule::new(api.clone());
    for &method_name in &methods {
        let name = Arc::new(method_name.to_string());
        passthrough_module.register_async_method(method_name, move |params, api, _| {
            let name = name.clone();
            async move {
                let params_vec: Vec<Value> = params.parse().unwrap_or_default();
                api.builder_client()
                    .request::<Value, Vec<Value>>(&name, params_vec)
                    .await
                    .map_err(|e| {
                        ErrorObjectOwned::from(ErrorObject::owned(
                            -32000,
                            e.to_string(),
                            None::<()>,
                        ))
                    })
            }
        })?;
    }

    let mut module = RpcModule::new(api.clone());
    module.merge(mod_tx_send)?;
    module.merge(mod_txs_raw)?;
    module.merge(mod_tx_get)?;
    module.merge(passthrough_module)?;

    // Start JSON-RPC server
    let addr: SocketAddr = "0.0.0.0:1545".parse()?;
    // let rpc_middleware = jsonrpsee::server::middleware::rpc::RpcServiceBuilder::new().layer_fn(LoggingMiddleware);
    let server = ServerBuilder::default().build(addr).await?;
    let handle = server.start(module);
    println!("🚀 JSON-RPC server listening on http://{}", addr);
    handle.stopped().await;

    Ok(())
}

#[allow(dead_code)]
struct LoggingMiddleware<S>(S);

impl<S> RpcServiceT for LoggingMiddleware<S>
where
	S: RpcServiceT,
{
	type MethodResponse = S::MethodResponse;
	type NotificationResponse = S::NotificationResponse;
	type BatchResponse = S::BatchResponse;

	fn call<'a>(&self, request: Request<'a>) -> impl Future<Output = Self::MethodResponse> + Send + 'a {
		if !request.method_name().contains("tog") {
            println!("Received request: {:?}", request);
        }
		self.0.call(request)
	}

	fn batch<'a>(&self, batch: Batch<'a>) -> impl Future<Output = Self::BatchResponse> + Send + 'a {
		println!("Received batch: {:?}", batch);
		self.0.batch(batch)
	}

	fn notification<'a>(&self, n: Notification<'a>) -> impl Future<Output = Self::NotificationResponse> + Send + 'a {
		println!("Received notif: {:?}", n);
		self.0.notification(n)
	}
}
