mod rpc;
mod noop;

use std::{fs, net::SocketAddr, sync::Arc};
use alloy_primitives::{Bytes, hex};
use jsonrpsee::server::{ServerBuilder};
use jsonrpsee::RpcModule;
use jsonrpsee_types::ErrorObjectOwned;
use reth_optimism_chainspec::OpChainSpec;
use reth_optimism_primitives::OpPrimitives;
use reth_optimism_txpool::{OpPooledTransaction, OpTransactionPool, OpTransactionValidator};
// use reth_storage_api::noop::NoopProvider;
use reth_tasks::TokioTaskExecutor;
use reth_transaction_pool::{
    blobstore::NoopBlobStore, CoinbaseTipOrdering, PoolConfig, TransactionValidationTaskExecutor,
};
use alloy_genesis::Genesis;

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
    // let provider = NoopProvider::<OpChainSpec, OpPrimitives>::new(Arc::new(chain_spec));

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
    
    let api = Arc::new(GuarantorApi::new(pool, provider));
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
    
    let mut mod_tx_get = RpcModule::new(api.clone());
    mod_tx_get
        .register_async_method("tog_getBestTransactionHashes", |_, api, _| async move {
            api.get_best_transaction_hashes().await
        })?;

    let mut module = RpcModule::new(api.clone());
    module.merge(mod_tx_send)?;
    module.merge(mod_tx_get)?;

    // Start JSON-RPC server
    let addr: SocketAddr = "0.0.0.0:1545".parse()?;
    let server = ServerBuilder::default().build(addr).await?;
    let handle = server.start(module);
    println!("🚀 JSON-RPC server listening on http://{}", addr);
    handle.stopped().await;

    Ok(())
}
