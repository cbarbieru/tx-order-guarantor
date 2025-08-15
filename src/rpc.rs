use std::sync::Arc;
use alloy_consensus::Transaction;
use alloy_primitives::{Bytes, B256};
use jsonrpsee_core::RpcResult;
use reth_optimism_chainspec::OpChainSpec;
use reth_optimism_primitives::OpPrimitives;
use reth_optimism_txpool::conditional::MaybeConditionalTransaction;
use reth_rpc_eth_types::EthApiError;
use reth_rpc_eth_types::utils::recover_raw_transaction;
use reth_transaction_pool::{PoolTransaction, TransactionOrigin, TransactionPool};
use crate::NoopProviderTog;

#[derive(Clone, Debug)]
pub struct GuarantorApi<Pool, Provider = NoopProviderTog<OpChainSpec, OpPrimitives>> {
    inner: Arc<OpEthExtApiInner<Pool>>,
    provider: Provider,
}

impl<Pool> GuarantorApi<Pool>
where
    Pool: TransactionPool<Transaction: MaybeConditionalTransaction> + 'static,
{
    /// Creates a new [`OpEthExtApi`].
    pub fn new(pool: Pool, provider: NoopProviderTog<OpChainSpec, OpPrimitives>) -> Self {
        let inner = Arc::new(OpEthExtApiInner::new(pool));
        Self { inner, provider }
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
    async fn get_best_transaction_hashes(&self) -> RpcResult<Vec<B256>>;
}

#[async_trait::async_trait]
impl<Pool> GuarantorTxGet for GuarantorApi<Pool>
where
    Pool: TransactionPool<Transaction: MaybeConditionalTransaction> + 'static,
{
    async fn send_raw_transaction(&self, bytes: Bytes) -> RpcResult<B256> {

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

    async fn get_best_transaction_hashes(&self) -> RpcResult<Vec<B256>> {
        let best_txs = self.best_transactions();
        let hashes = best_txs.into_iter().map(|tx| tx.hash().clone()).collect();
        Ok(hashes)
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
