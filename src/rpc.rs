//! Eth API extension.

use alloy_primitives::{Bytes, B256};
use alloy_rpc_types_eth::erc4337::{TransactionConditional};
use jsonrpsee_core::RpcResult;
use reth_optimism_txpool::conditional::MaybeConditionalTransaction;
use reth_rpc_eth_api::L2EthApiExtServer;
use reth_rpc_eth_types::EthApiError;
use reth_rpc_eth_types::utils::recover_raw_transaction;
use reth_transaction_pool::{PoolTransaction, TransactionOrigin, TransactionPool};
use reth_storage_api::{BlockReaderIdExt, StateProviderFactory};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct GuarantorApi<Pool, Provider> {
    inner: Arc<OpEthExtApiInner<Pool, Provider>>,
}

impl<Pool, Provider> GuarantorApi<Pool, Provider>
where
    Provider: BlockReaderIdExt + StateProviderFactory + Clone + 'static,
    Pool: TransactionPool<Transaction: MaybeConditionalTransaction> + 'static,
{
    /// Creates a new [`OpEthExtApi`].
    pub fn new(pool: Pool, provider: Provider) -> Self {
        let inner = Arc::new(OpEthExtApiInner::new(pool, provider));
        Self { inner }
    }

    pub fn best_transactions(&self) -> Vec<Pool::Transaction> {
        self.inner.best_transactions()
    }

    #[inline]
    fn pool(&self) -> &Pool {
        self.inner.pool()
    }

    #[inline]
    fn provider(&self) -> &Provider {
        self.inner.provider()
    }
}

#[async_trait::async_trait]
impl<Pool, Provider> L2EthApiExtServer for GuarantorApi<Pool, Provider>
where
    Provider: BlockReaderIdExt + StateProviderFactory + Clone + 'static,
    Pool: TransactionPool<Transaction: MaybeConditionalTransaction> + 'static,
{
    async fn send_raw_transaction_conditional(
        &self,
        bytes: Bytes,
        condition: TransactionConditional,
    ) -> RpcResult<B256> {

        let recovered_tx = recover_raw_transaction(&bytes).map_err(|_| {
            OpEthApiError::Eth(EthApiError::FailedToDecodeSignedTransaction)
        })?;

        let tx = <Pool as TransactionPool>::Transaction::from_pooled(recovered_tx);

        let hash =
            self.pool().add_transaction(TransactionOrigin::Private, tx).await.map_err(|e| {
                    OpEthApiError::Eth(EthApiError::PoolError(e.into()))
            })?;

        Ok(hash)
    }
}

#[async_trait::async_trait]
pub trait GuarantorTxGet {
    async fn get_best_transaction_hashes(&self) -> RpcResult<Vec<B256>>;
}

#[async_trait::async_trait]
impl<Pool, Provider> GuarantorTxGet for GuarantorApi<Pool, Provider>
where
    Provider: BlockReaderIdExt + StateProviderFactory + Clone + 'static,
    Pool: TransactionPool<Transaction: MaybeConditionalTransaction> + 'static,
{
    async fn get_best_transaction_hashes(&self) -> RpcResult<Vec<B256>> {
        let best_txs = self.best_transactions();
        let hashes = best_txs.into_iter().map(|tx| tx.hash().clone()).collect();
        Ok(hashes)
    }
}

#[derive(Debug)]
pub struct OpEthExtApiInner<Pool, Provider> {
    /// The transaction pool of the node.
    pool: Pool,
    provider: Provider,
}

impl<Pool, Provider> OpEthExtApiInner<Pool, Provider>
where 
    Pool: TransactionPool<Transaction: MaybeConditionalTransaction> + 'static
{
    pub fn new(pool: Pool, provider: Provider) -> Self {
        Self {
            pool,
            provider
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

    #[inline]
    const fn provider(&self) -> &Provider {
        &self.provider
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
