mod rpc;

use std::{fs, net::SocketAddr, sync::Arc};

use alloy_primitives::Bytes;
use alloy_rpc_types_eth::erc4337::{TransactionConditional};
use jsonrpsee::server::{ServerBuilder};
use jsonrpsee::RpcModule;
use reth_optimism_chainspec::OpChainSpec;
use reth_optimism_primitives::OpPrimitives;
use reth_optimism_txpool::{OpPooledTransaction, OpTransactionPool, OpTransactionValidator};
use reth_rpc_eth_api::L2EthApiExtServer;
use reth_storage_api::noop::NoopProvider;
use reth_transaction_pool::{
    blobstore::NoopBlobStore, CoinbaseTipOrdering, PoolConfig, TransactionValidationTaskExecutor,
};
use alloy_genesis::Genesis;

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

    let chain_spec = load_chain_spec_from_file("../../res/l2-genesis.json")?;
    
    let provider = NoopProvider::<OpChainSpec, OpPrimitives>::new(Arc::new(chain_spec));

    let blob_store = NoopBlobStore::default();

    let eth_validator = TransactionValidationTaskExecutor::eth_builder(provider.clone()) 
    .build::<OpPooledTransaction, _>(blob_store.clone());
    
    let op_validator = OpTransactionValidator::new(eth_validator);
    
    let validation_executor = TransactionValidationTaskExecutor::new(op_validator);
    
    let ordering = CoinbaseTipOrdering::<OpPooledTransaction>::default();
    
    let config = PoolConfig::default();

    let pool: Arc<OpTransactionPool<_, _, _>> =
        Arc::new(OpTransactionPool::new(validation_executor, ordering, blob_store.clone(), config));

    let api = GuarantorApi::new(pool.clone(), provider.clone());

    let mut mod_tx_send = RpcModule::new(api.clone());
    mod_tx_send
        .register_async_method("send_raw_transaction_conditional", |params, api, _| async move {
            // parse params and call your method
            let (bytes, condition): (Bytes, TransactionConditional) = params.parse()?;
            api.send_raw_transaction_conditional(bytes, condition).await
        })?;
    
    let mut mod_tx_get = RpcModule::new(api.clone());
    mod_tx_get
        .register_async_method("get_best_transaction_hashes", |_, api, _| async move {
            api.get_best_transaction_hashes().await
        })?;

    let mut module = RpcModule::new(api.clone());
    module.merge(mod_tx_send)?;
    module.merge(mod_tx_get)?;

    // Start JSON-RPC server
    let addr: SocketAddr = "127.0.0.1:8545".parse()?;
    let server = ServerBuilder::default().build(addr).await?;
    server.start(module);
    println!("🚀 JSON-RPC server listening on http://{}", addr);

    Ok(())
}
