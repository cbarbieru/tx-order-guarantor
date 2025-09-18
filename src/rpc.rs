use std::sync::Arc;
use tokio::sync::Mutex;
use alloy_consensus::Transaction;
use alloy_primitives::{Bytes, B256, hex};
use jsonrpsee_core::RpcResult;
use jsonrpsee::http_client::HttpClient;
use jsonrpsee::core::client::{ClientT, Error};
use jsonrpsee::types::ErrorObject;
use jsonrpsee::core::middleware::{Batch, Notification, RpcServiceT};
use jsonrpsee::types::Request;
use reth_optimism_chainspec::OpChainSpec;
use reth_optimism_primitives::OpPrimitives;
use reth_optimism_txpool::conditional::MaybeConditionalTransaction;
use reth_rpc_eth_types::EthApiError;
use reth_rpc_eth_types::utils::recover_raw_transaction;
use reth_transaction_pool::{PoolTransaction, TransactionOrigin, TransactionPool};
use crate::NoopProviderTog;

#[derive(Clone, Debug)]
pub struct GuarantorApi<Pool, S, Provider = NoopProviderTog<OpChainSpec, OpPrimitives>> {
    inner: Arc<OpEthExtApiInner<Pool>>,
    provider: Provider,
    transactions: Arc<Mutex<Vec<Bytes>>>,
    builder_client: Arc<HttpClient>,
    service: Option<S>,
}

impl<Pool, S> GuarantorApi<Pool, S>
where
    Pool: TransactionPool<Transaction: MaybeConditionalTransaction> + 'static,
    S: RpcServiceT + Send + Sync + Clone + 'static,
{
    /// Creates a new [`OpEthExtApi`].
    pub fn new(pool: Pool, provider: NoopProviderTog<OpChainSpec, OpPrimitives>, builder_client: HttpClient) -> Self {
        let inner = Arc::new(OpEthExtApiInner::new(pool));
        let transactions = Arc::new(Mutex::new(Vec::new()));
        let builder_client = Arc::new(builder_client);
        Self { inner, provider, transactions, builder_client, service: None }
    }

    pub fn best_transactions(&self) -> Vec<Pool::Transaction> {
        self.inner.best_transactions()
    }

    #[inline]
    fn pool(&self) -> &Pool {
        self.inner.pool()
    }

}

#[async_trait::async_trait]
pub trait GuarantorTxGet {
    async fn send_raw_transaction(&self, bytes: Bytes) -> RpcResult<B256>;
    async fn get_raw_transactions(&self) -> RpcResult<Vec<String>>;
    async fn get_best_transaction_hashes(&self) -> RpcResult<Vec<B256>>;
}

#[async_trait::async_trait]
impl<Pool, S> GuarantorTxGet for GuarantorApi<Pool, S>
where
    Pool: TransactionPool<Transaction: MaybeConditionalTransaction> + 'static,
    S: RpcServiceT + Send + Sync + Clone + 'static,
{
    async fn send_raw_transaction(&self, bytes: Bytes) -> RpcResult<B256> {
        let mut txs = self.transactions.lock().await;
        txs.push(bytes.clone());
        drop(txs);

        let recovered_tx = recover_raw_transaction(&bytes).map_err(|_| {
            OpEthApiError::Eth(EthApiError::FailedToDecodeSignedTransaction)
        })?;
        println!("tx = {:?}", recovered_tx);

        let tx = <Pool as TransactionPool>::Transaction::from_pooled(recovered_tx);

        let nonce = tx.clone().nonce();
        self.provider.set_nonce_updater(move || { nonce });
        
        let hash =
            self.pool().add_transaction(TransactionOrigin::Private, tx).await.map_err(|e| {
                    OpEthApiError::Eth(EthApiError::PoolError(e.into()))
            })?;

        println!("hash = {:?}", &hash);
        Ok(hash)
    }

    async fn get_raw_transactions(&self) -> RpcResult<Vec<String>> {
        let mut txs = self.transactions.lock().await;
        let raw_hex: Vec<String> = txs
            .iter()
            .map(|bytes| format!("0x{}", hex::encode(bytes)))
            .collect();
        txs.clear();
        drop(txs);

        println!("raw_tx_count = {}", raw_hex.len());
        Ok(raw_hex)
    }

    async fn get_best_transaction_hashes(&self) -> RpcResult<Vec<B256>> {
        let best_txs = self.best_transactions();
        let hashes = best_txs.into_iter().map(|tx| tx.hash().clone()).collect();
        println!("ordered_hashes = {:?}", &hashes);
        Ok(hashes)
    }
}

impl<Pool, S> RpcServiceT for GuarantorApi<Pool, S>
where
    Pool: TransactionPool<Transaction: MaybeConditionalTransaction> + 'static,
	S: RpcServiceT + Send + Sync + Clone + 'static,
{
    type MethodResponse = S::MethodResponse;
	type NotificationResponse = S::NotificationResponse;
	type BatchResponse = S::BatchResponse;

    fn call<'a>(&self, req: Request<'a>) -> impl Future<Output = Self::MethodResponse> + Send + 'a {
        Box::pin(async move {
            if req.method.starts_with("tog") {
                self.service.as_ref().expect("service not set").call(req).await
            } else {
                let params_ser = req.params.into();

                let response: RpcResult<serde_json::Value> = self.builder_client.as_ref().request(&req.method, params_ser).await.map_err(client_error);
                match response {
                    Ok(value) => {
                        Self::MethodResponse::success(request_id, value)
                    }
                    Err(e) => {
                        Self::MethodResponse::error(request_id, e.into())
                    }
                }    
            }
        })
	}

	fn batch<'a>(&self, batch: Batch<'a>) -> impl Future<Output = Self::BatchResponse> + Send + 'a {
		print!("batch");
        self.service.as_ref().expect("service not set").batch(batch)
	}

	fn notification<'a>(&self, n: Notification<'a>) -> impl Future<Output = Self::NotificationResponse> + Send + 'a {
		print!("notification");
        self.service.as_ref().expect("service not set").notification(n)
	}
}

#[derive(Debug)]
pub struct OpEthExtApiInner<Pool> {
    /// The transaction pool of the node.
    pool: Pool,
}

impl<Pool> OpEthExtApiInner<Pool>
where 
    Pool: TransactionPool<Transaction: MaybeConditionalTransaction> + 'static
{
    pub fn new(pool: Pool) -> Self {
        Self {
            pool,
        }
    }

    pub fn best_transactions(&self) -> Vec<Pool::Transaction> {
        self.pool().best_transactions()
            .map(|pending_tx| pending_tx.transaction.clone())
            .collect()
    }

    #[inline]
    const fn pool(&self) -> &Pool {
        &self.pool
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OpEthApiError {
    #[error(transparent)]
    Eth(#[from] EthApiError),
}

impl From<OpEthApiError> for jsonrpsee_types::error::ErrorObject<'static> {
    fn from(err: OpEthApiError) -> Self {
        match err {
            OpEthApiError::Eth(err) => err.into(),
        }
    }
}

fn client_error(e: impl ToString) -> ErrorObject<'static> {
    ErrorObject::owned(-32000, e.to_string(), None::<()>)
}
