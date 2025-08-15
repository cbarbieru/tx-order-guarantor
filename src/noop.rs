use std::{fmt, sync::{Arc, Mutex}};
use alloy_primitives::{Address, B256, U256, BlockHash, BlockNumber, TxHash, TxNumber, StorageKey, StorageValue, Bytes };
use alloy_eips::{BlockHashOrNumber, BlockId, BlockNumberOrTag};
use alloy_consensus::transaction::TransactionMeta;
use reth_primitives_traits::{Account, NodePrimitives, RecoveredBlock, SealedHeader, Bytecode};
use reth_ethereum_primitives::EthPrimitives;
use reth_storage_api::{
    AccountReader, BlockSource, BlockBodyIndicesProvider, BlockReader, BlockNumReader, BlockHashReader, StateProofProvider,
    BlockIdReader, BlockReaderIdExt, HeaderProvider, ReceiptProvider, ReceiptProviderIdExt, StateProvider, StateProviderBox, StateRootProvider,
    StateProviderFactory, TransactionVariant, TransactionsProvider, HashedPostStateProvider, BytecodeReader, StorageRootProvider
};
use reth_db_models::{ StoredBlockBodyIndices};
use reth_errors::{ProviderResult, ProviderError};
use reth_chainspec::{ChainInfo, ChainSpecProvider, EthChainSpec};
use reth_trie_common::{
    updates::TrieUpdates, AccountProof, HashedPostState, HashedStorage, MultiProof,
    MultiProofTargets, StorageMultiProof, StorageProof, TrieInput,
};
use core::{
    fmt::Debug,
    marker::PhantomData,
    ops::{RangeBounds, RangeInclusive},
};

#[non_exhaustive]
pub struct NoopProviderTog<ChainSpec = reth_chainspec::ChainSpec, N = EthPrimitives> {
    chain_spec: Arc<ChainSpec>,
    _phantom: PhantomData<N>,
    nonce_updater: Arc<Mutex<Box<dyn FnMut() -> u64 + Send + 'static>>>,
}

impl<CS, P> NoopProviderTog<CS, P> {
    pub fn new(chain_spec: Arc<CS>) -> Self {
        Self {
            chain_spec,
            _phantom: Default::default(),
            nonce_updater: Arc::new(Mutex::new(Box::new(|| { 0 }))),
        }
    }
    pub fn set_nonce_updater(&self, new_updater: impl FnMut() -> u64 + Send + 'static) {
        *self.nonce_updater.lock().unwrap() = Box::new(new_updater);
    }
}

impl<CS, N> Clone for NoopProviderTog<CS, N> {
    fn clone(&self) -> Self {
        Self {
            chain_spec: Arc::clone(&self.chain_spec),
            _phantom: Default::default(),
            nonce_updater: Arc::clone(&self.nonce_updater),
        }
    }
}

impl<C: Send + Sync, N: NodePrimitives> AccountReader for NoopProviderTog<C, N> {
    fn basic_account(&self, _address: &Address) -> ProviderResult<Option<Account>> {
        let mut updater = self.nonce_updater.lock().unwrap();
        let nonce = updater.as_mut()();
        Ok(Some(Account {
            nonce: nonce,
            balance: U256::MAX,
            bytecode_hash: None,
        }))
    }
}

impl<C: Send + Sync, N: NodePrimitives> HeaderProvider for NoopProviderTog<C, N> {
    type Header = N::BlockHeader;

    fn header(&self, _block_hash: &BlockHash) -> ProviderResult<Option<Self::Header>> {
        Ok(None)
    }

    fn header_by_number(&self, _num: u64) -> ProviderResult<Option<Self::Header>> {
        Ok(None)
    }

    fn header_td(&self, _hash: &BlockHash) -> ProviderResult<Option<U256>> {
        Ok(None)
    }

    fn header_td_by_number(&self, _number: BlockNumber) -> ProviderResult<Option<U256>> {
        Ok(None)
    }

    fn headers_range(
        &self,
        _range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<Self::Header>> {
        Ok(Vec::new())
    }

    fn sealed_header(
        &self,
        _number: BlockNumber,
    ) -> ProviderResult<Option<SealedHeader<Self::Header>>> {
        Ok(None)
    }

    fn sealed_headers_while(
        &self,
        _range: impl RangeBounds<BlockNumber>,
        _predicate: impl FnMut(&SealedHeader<Self::Header>) -> bool,
    ) -> ProviderResult<Vec<SealedHeader<Self::Header>>> {
        Ok(Vec::new())
    }
}

impl<C: Send + Sync, N: NodePrimitives> BlockReader for NoopProviderTog<C, N> {
    type Block = N::Block;

    fn find_block_by_hash(
        &self,
        _hash: B256,
        _source: BlockSource,
    ) -> ProviderResult<Option<Self::Block>> {
        Ok(None)
    }

    fn block(&self, _id: BlockHashOrNumber) -> ProviderResult<Option<Self::Block>> {
        Ok(None)
    }

    fn pending_block(&self) -> ProviderResult<Option<RecoveredBlock<Self::Block>>> {
        Ok(None)
    }

    fn pending_block_and_receipts(
        &self,
    ) -> ProviderResult<Option<(RecoveredBlock<Self::Block>, Vec<Self::Receipt>)>> {
        Ok(None)
    }

    fn recovered_block(
        &self,
        _id: BlockHashOrNumber,
        _transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<RecoveredBlock<Self::Block>>> {
        Ok(None)
    }

    fn sealed_block_with_senders(
        &self,
        _id: BlockHashOrNumber,
        _transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<RecoveredBlock<Self::Block>>> {
        Ok(None)
    }

    fn block_range(&self, _range: RangeInclusive<BlockNumber>) -> ProviderResult<Vec<Self::Block>> {
        Ok(Vec::new())
    }

    fn block_with_senders_range(
        &self,
        _range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<RecoveredBlock<Self::Block>>> {
        Ok(Vec::new())
    }

    fn recovered_block_range(
        &self,
        _range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<RecoveredBlock<Self::Block>>> {
        Ok(Vec::new())
    }
}

impl<C: Send + Sync, N: NodePrimitives> BlockIdReader for NoopProviderTog<C, N> {
    fn pending_block_num_hash(&self) -> ProviderResult<Option<alloy_eips::BlockNumHash>> {
        Ok(None)
    }

    fn safe_block_num_hash(&self) -> ProviderResult<Option<alloy_eips::BlockNumHash>> {
        Ok(None)
    }

    fn finalized_block_num_hash(&self) -> ProviderResult<Option<alloy_eips::BlockNumHash>> {
        Ok(None)
    }
}

impl<C: Send + Sync, N: NodePrimitives> BlockReaderIdExt for NoopProviderTog<C, N> {
    fn block_by_id(&self, _id: BlockId) -> ProviderResult<Option<N::Block>> {
        Ok(None)
    }

    fn sealed_header_by_id(
        &self,
        _id: BlockId,
    ) -> ProviderResult<Option<SealedHeader<N::BlockHeader>>> {
        Ok(None)
    }

    fn header_by_id(&self, _id: BlockId) -> ProviderResult<Option<N::BlockHeader>> {
        Ok(None)
    }
}

impl<ChainSpec: Send + Sync, N: Send + Sync> BlockNumReader for NoopProviderTog<ChainSpec, N> {
    fn chain_info(&self) -> ProviderResult<ChainInfo> {
        Ok(ChainInfo::default())
    }

    fn best_block_number(&self) -> ProviderResult<BlockNumber> {
        Ok(0)
    }

    fn last_block_number(&self) -> ProviderResult<BlockNumber> {
        Ok(0)
    }

    fn block_number(&self, _hash: B256) -> ProviderResult<Option<BlockNumber>> {
        Ok(None)
    }
}

impl<ChainSpec: Send + Sync, N: Send + Sync> BlockHashReader for NoopProviderTog<ChainSpec, N> {
    fn block_hash(&self, _number: u64) -> ProviderResult<Option<B256>> {
        Ok(None)
    }

    fn canonical_hashes_range(
        &self,
        _start: BlockNumber,
        _end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        Ok(Vec::new())
    }
}

impl<C: Send + Sync, N: Send + Sync> BlockBodyIndicesProvider for NoopProviderTog<C, N> {
    fn block_body_indices(&self, _num: u64) -> ProviderResult<Option<StoredBlockBodyIndices>> {
        Ok(None)
    }

    fn block_body_indices_range(
        &self,
        _range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<StoredBlockBodyIndices>> {
        Ok(Vec::new())
    }
}

impl<C: Send + Sync, N: NodePrimitives> TransactionsProvider for NoopProviderTog<C, N> {
    type Transaction = N::SignedTx;

    fn transaction_id(&self, _tx_hash: TxHash) -> ProviderResult<Option<TxNumber>> {
        Ok(None)
    }

    fn transaction_by_id(&self, _id: TxNumber) -> ProviderResult<Option<Self::Transaction>> {
        Ok(None)
    }

    fn transaction_by_id_unhashed(
        &self,
        _id: TxNumber,
    ) -> ProviderResult<Option<Self::Transaction>> {
        Ok(None)
    }

    fn transaction_by_hash(&self, _hash: TxHash) -> ProviderResult<Option<Self::Transaction>> {
        Ok(None)
    }

    fn transaction_by_hash_with_meta(
        &self,
        _hash: TxHash,
    ) -> ProviderResult<Option<(Self::Transaction, TransactionMeta)>> {
        Ok(None)
    }

    fn transaction_block(&self, _id: TxNumber) -> ProviderResult<Option<BlockNumber>> {
        todo!()
    }

    fn transactions_by_block(
        &self,
        _block_id: BlockHashOrNumber,
    ) -> ProviderResult<Option<Vec<Self::Transaction>>> {
        Ok(None)
    }

    fn transactions_by_block_range(
        &self,
        _range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<Vec<Self::Transaction>>> {
        Ok(Vec::default())
    }

    fn transactions_by_tx_range(
        &self,
        _range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Self::Transaction>> {
        Ok(Vec::default())
    }

    fn senders_by_tx_range(
        &self,
        _range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Address>> {
        Ok(Vec::default())
    }

    fn transaction_sender(&self, _id: TxNumber) -> ProviderResult<Option<Address>> {
        Ok(None)
    }
}

impl<C: Send + Sync, N: NodePrimitives> ReceiptProvider for NoopProviderTog<C, N> {
    type Receipt = N::Receipt;

    fn receipt(&self, _id: TxNumber) -> ProviderResult<Option<Self::Receipt>> {
        Ok(None)
    }

    fn receipt_by_hash(&self, _hash: TxHash) -> ProviderResult<Option<Self::Receipt>> {
        Ok(None)
    }

    fn receipts_by_block(
        &self,
        _block: BlockHashOrNumber,
    ) -> ProviderResult<Option<Vec<Self::Receipt>>> {
        Ok(None)
    }

    fn receipts_by_tx_range(
        &self,
        _range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Self::Receipt>> {
        Ok(Vec::new())
    }

    fn receipts_by_block_range(
        &self,
        _block_range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<Vec<Self::Receipt>>> {
        Ok(Vec::new())
    }
}

impl<C: Send + Sync, N: NodePrimitives> ReceiptProviderIdExt for NoopProviderTog<C, N> {}

impl<C: Send + Sync, N: NodePrimitives> HashedPostStateProvider for NoopProviderTog<C, N> {
    fn hashed_post_state(&self, _bundle_state: &revm_database::BundleState) -> HashedPostState {
        HashedPostState::default()
    }
}

impl<C: Send + Sync, N: NodePrimitives> StateProofProvider for NoopProviderTog<C, N> {
    fn proof(
        &self,
        _input: TrieInput,
        address: Address,
        _slots: &[B256],
    ) -> ProviderResult<AccountProof> {
        Ok(AccountProof::new(address))
    }

    fn multiproof(
        &self,
        _input: TrieInput,
        _targets: MultiProofTargets,
    ) -> ProviderResult<MultiProof> {
        Ok(MultiProof::default())
    }

    fn witness(&self, _input: TrieInput, _target: HashedPostState) -> ProviderResult<Vec<Bytes>> {
        Ok(Vec::default())
    }
}

impl<C: Send + Sync, N: NodePrimitives> StorageRootProvider for NoopProviderTog<C, N> {
    fn storage_root(
        &self,
        _address: Address,
        _hashed_storage: HashedStorage,
    ) -> ProviderResult<B256> {
        Ok(B256::default())
    }

    fn storage_proof(
        &self,
        _address: Address,
        slot: B256,
        _hashed_storage: HashedStorage,
    ) -> ProviderResult<StorageProof> {
        Ok(StorageProof::new(slot))
    }

    fn storage_multiproof(
        &self,
        _address: Address,
        _slots: &[B256],
        _hashed_storage: HashedStorage,
    ) -> ProviderResult<StorageMultiProof> {
        Ok(StorageMultiProof::empty())
    }
}

impl<C: Send + Sync, N: NodePrimitives> StateRootProvider for NoopProviderTog<C, N> {
    fn state_root(&self, _state: HashedPostState) -> ProviderResult<B256> {
        Ok(B256::default())
    }

    fn state_root_from_nodes(&self, _input: TrieInput) -> ProviderResult<B256> {
        Ok(B256::default())
    }

    fn state_root_with_updates(
        &self,
        _state: HashedPostState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        Ok((B256::default(), TrieUpdates::default()))
    }

    fn state_root_from_nodes_with_updates(
        &self,
        _input: TrieInput,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        Ok((B256::default(), TrieUpdates::default()))
    }
}

impl<C: Send + Sync, N: NodePrimitives> StateProvider for NoopProviderTog<C, N> {
    fn storage(
        &self,
        _account: Address,
        _storage_key: StorageKey,
    ) -> ProviderResult<Option<StorageValue>> {
        Ok(None)
    }
}

impl<C: Send + Sync, N: NodePrimitives> BytecodeReader for NoopProviderTog<C, N> {
    fn bytecode_by_hash(&self, _code_hash: &B256) -> ProviderResult<Option<Bytecode>> {
        Ok(None)
    }
}

impl<C: Send + Sync + 'static, N: NodePrimitives> StateProviderFactory for NoopProviderTog<C, N> {
    fn latest(&self) -> ProviderResult<StateProviderBox> {
        Ok(Box::new(self.clone()))
    }

    fn state_by_block_number_or_tag(
        &self,
        number_or_tag: BlockNumberOrTag,
    ) -> ProviderResult<StateProviderBox> {
        match number_or_tag {
            BlockNumberOrTag::Latest => self.latest(),
            BlockNumberOrTag::Finalized => {
                // we can only get the finalized state by hash, not by num
                let hash =
                    self.finalized_block_hash()?.ok_or(ProviderError::FinalizedBlockNotFound)?;

                // only look at historical state
                self.history_by_block_hash(hash)
            }
            BlockNumberOrTag::Safe => {
                // we can only get the safe state by hash, not by num
                let hash = self.safe_block_hash()?.ok_or(ProviderError::SafeBlockNotFound)?;

                self.history_by_block_hash(hash)
            }
            BlockNumberOrTag::Earliest => {
                self.history_by_block_number(self.earliest_block_number()?)
            }
            BlockNumberOrTag::Pending => self.pending(),
            BlockNumberOrTag::Number(num) => self.history_by_block_number(num),
        }
    }

    fn history_by_block_number(&self, _block: BlockNumber) -> ProviderResult<StateProviderBox> {
        Ok(Box::new(self.clone()))
    }

    fn history_by_block_hash(&self, _block: BlockHash) -> ProviderResult<StateProviderBox> {
        Ok(Box::new(self.clone()))
    }

    fn state_by_block_hash(&self, _block: BlockHash) -> ProviderResult<StateProviderBox> {
        Ok(Box::new(self.clone()))
    }

    fn pending(&self) -> ProviderResult<StateProviderBox> {
        Ok(Box::new(self.clone()))
    }

    fn pending_state_by_hash(&self, _block_hash: B256) -> ProviderResult<Option<StateProviderBox>> {
        Ok(Some(Box::new(self.clone())))
    }
}

impl<C, N> fmt::Debug for NoopProviderTog<C, N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // You can still debug other fields, but you must exclude nonce_updater
        f.debug_struct("NoopProviderTog")
            // .field("nonce_updater", &self.nonce_updater) // DO NOT DO THIS
            .finish()
    }
}

impl<ChainSpec: EthChainSpec + 'static, N: Debug + Send + Sync + 'static> ChainSpecProvider
    for NoopProviderTog<ChainSpec, N>
{
    type ChainSpec = ChainSpec;

    fn chain_spec(&self) -> Arc<Self::ChainSpec> {
        self.chain_spec.clone()
    }
}

