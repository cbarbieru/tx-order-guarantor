use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use alloy_consensus::{Transaction, TxEnvelope};
use alloy_consensus::transaction::SignerRecoverable;
use alloy_eips::eip2718::Decodable2718;
use alloy_primitives::{Address, B256, Bytes, keccak256};

#[derive(Debug, thiserror::Error)]
pub enum MempoolError {
    #[error("could not decode 2718 transaction: {0}")]
    Decode(String),
    #[error("could not recover signer: {0}")]
    Recovery(String),
}

/// One transaction held in the enclave pool.
#[derive(Debug, Clone)]
struct PoolEntry {
    hash: B256,
    sender: Address,
    nonce: u64,
    /// Effective tip per gas at the configured base fee. `None` for txs that
    /// can't pay the base fee (kept, but sorted last).
    effective_tip: u128,
    /// Insertion order — deterministic tie-break (first-seen wins).
    seq: u64,
}

/// In-enclave transaction pool + deterministic tip ordering.
///
/// Mirrors the original guarantor's two side effects faithfully:
///   * the raw buffer is **drained** on every `get_raw_transactions`, and
///   * the ordered set is **cleared every `clear_every` calls** to
///     `best_transaction_hashes` (the legacy "clear once every 7 blocks").
///
/// Both behaviours are configurable so you can revisit them — they were quirks
/// of the original, not load-bearing invariants.
#[derive(Debug)]
pub struct Mempool {
    /// Base fee used to compute effective tips. The builder's pending base fee
    /// should be fed in here; 0 makes tip == priority fee.
    base_fee: u64,
    /// Drained wholesale by `get_raw_transactions` (legacy side buffer).
    raw_buffer: Vec<Bytes>,
    /// The ordered working set, keyed by hash for dedup.
    entries: HashMap<B256, PoolEntry>,
    next_seq: u64,
    best_calls: u64,
    clear_every: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolStats {
    pub raw_buffered: usize,
    pub pooled: usize,
    pub best_calls: u64,
}

impl Default for Mempool {
    fn default() -> Self {
        Self::new(0, 7)
    }
}

impl Mempool {
    /// `clear_every == 0` disables the periodic clear.
    pub fn new(base_fee: u64, clear_every: u64) -> Self {
        Self {
            base_fee,
            raw_buffer: Vec::new(),
            entries: HashMap::new(),
            next_seq: 0,
            best_calls: 0,
            clear_every,
        }
    }

    pub fn set_base_fee(&mut self, base_fee: u64) {
        self.base_fee = base_fee;
    }

    pub fn stats(&self) -> PoolStats {
        PoolStats {
            raw_buffered: self.raw_buffer.len(),
            pooled: self.entries.len(),
            best_calls: self.best_calls,
        }
    }

    /// `eth_sendRawTransaction`: decode, recover signer, insert. Returns the
    /// transaction hash. The raw bytes go into both the drain buffer and the
    /// ordered set, exactly like the original.
    pub fn add_raw_transaction(&mut self, raw: Bytes) -> Result<B256, MempoolError> {
        // The canonical tx hash is keccak256 of the EIP-2718 encoding, which is
        // precisely the bytes the submitter sent — hash them directly so we are
        // faithful even to non-canonical re-encodings.
        let hash = keccak256(&raw);

        let mut slice: &[u8] = raw.as_ref();
        let envelope = TxEnvelope::decode_2718(&mut slice)
            .map_err(|e| MempoolError::Decode(e.to_string()))?;

        let sender = envelope
            .recover_signer()
            .map_err(|e| MempoolError::Recovery(e.to_string()))?;

        let effective_tip = envelope.effective_tip_per_gas(self.base_fee).unwrap_or(0);
        let nonce = envelope.nonce();

        let seq = self.next_seq;
        self.next_seq += 1;

        // Drain buffer keeps every submission in arrival order.
        self.raw_buffer.push(raw);

        // Ordered set dedups by hash (a resend doesn't reorder).
        self.entries.entry(hash).or_insert(PoolEntry {
            hash,
            sender,
            nonce,
            effective_tip,
            seq,
        });

        Ok(hash)
    }

    /// `tog_getRawTransactions`: drain and return arrival-order raw bytes.
    pub fn drain_raw_transactions(&mut self) -> Vec<Bytes> {
        std::mem::take(&mut self.raw_buffer)
    }

    /// `tog_getBestTransactionHashes`: deterministic tip-priority ordering that
    /// respects per-sender nonce order. Clears the ordered set every
    /// `clear_every` calls (legacy behaviour).
    pub fn best_transaction_hashes(&mut self) -> Vec<B256> {
        let ordered = self.compute_order();
        self.best_calls += 1;
        if self.clear_every != 0 && self.best_calls % self.clear_every == 0 {
            self.entries.clear();
        }
        ordered
    }

    /// Pure ordering computation (no side effects) — exposed for testing.
    fn compute_order(&self) -> Vec<B256> {
        // Group by sender, each group sorted by ascending nonce so the "head"
        // is always the next sequential tx for that sender.
        let mut by_sender: HashMap<Address, Vec<&PoolEntry>> = HashMap::new();
        for e in self.entries.values() {
            by_sender.entry(e.sender).or_default().push(e);
        }
        for q in by_sender.values_mut() {
            // nonce asc, then seq asc to make equal-nonce resends deterministic.
            q.sort_by(|a, b| a.nonce.cmp(&b.nonce).then(a.seq.cmp(&b.seq)));
        }

        // Greedy merge: repeatedly emit the ready head with the highest tip.
        // `heads[&sender]` is the index of that sender's next unemitted tx.
        let mut heads: HashMap<Address, usize> = by_sender.keys().map(|s| (*s, 0)).collect();
        let mut heap: BinaryHeap<HeapItem> = BinaryHeap::new();
        for (sender, q) in &by_sender {
            if let Some(head) = q.first() {
                heap.push(HeapItem::new(head, *sender));
            }
        }

        let mut out = Vec::with_capacity(self.entries.len());
        while let Some(item) = heap.pop() {
            out.push(item.hash);
            let q = &by_sender[&item.sender];
            let idx = heads.get_mut(&item.sender).unwrap();
            *idx += 1;
            if let Some(next) = q.get(*idx) {
                heap.push(HeapItem::new(next, item.sender));
            }
        }
        out
    }
}

/// Heap ordering: higher effective tip first; ties broken by earlier `seq`
/// (first-seen wins) so the output is fully deterministic.
struct HeapItem {
    effective_tip: u128,
    seq: u64,
    hash: B256,
    sender: Address,
}

impl HeapItem {
    fn new(e: &PoolEntry, sender: Address) -> Self {
        Self { effective_tip: e.effective_tip, seq: e.seq, hash: e.hash, sender }
    }
}

impl PartialEq for HeapItem {
    fn eq(&self, other: &Self) -> bool {
        self.effective_tip == other.effective_tip && self.seq == other.seq
    }
}
impl Eq for HeapItem {}
impl Ord for HeapItem {
    fn cmp(&self, other: &Self) -> Ordering {
        // Max-heap: greater == popped first.
        self.effective_tip
            .cmp(&other.effective_tip)
            // earlier seq should pop first, so reverse on seq.
            .then_with(|| other.seq.cmp(&self.seq))
    }
}
impl PartialOrd for HeapItem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{SignableTransaction, TxEip1559, TxEnvelope};
    use alloy_eips::eip2718::Encodable2718;
    use alloy_primitives::{B256, TxKind, U256};
    use alloy_signer::SignerSync;
    use alloy_signer_local::PrivateKeySigner;

    /// Build a signed EIP-1559 tx as the raw 2718 bytes a client would submit.
    fn raw_tx(key_byte: u8, nonce: u64, priority_fee: u128) -> Bytes {
        let signer = PrivateKeySigner::from_bytes(&B256::repeat_byte(key_byte)).unwrap();
        let tx = TxEip1559 {
            chain_id: 10,
            nonce,
            gas_limit: 21_000,
            max_fee_per_gas: priority_fee + 1_000_000_000,
            max_priority_fee_per_gas: priority_fee,
            to: TxKind::Call(Address::repeat_byte(0xab)),
            value: U256::ZERO,
            access_list: Default::default(),
            input: Default::default(),
        };
        let sig = signer.sign_hash_sync(&tx.signature_hash()).unwrap();
        let env: TxEnvelope = tx.into_signed(sig).into();
        Bytes::from(env.encoded_2718())
    }

    #[test]
    fn decodes_recovers_and_hashes() {
        let mut pool = Mempool::default();
        let raw = raw_tx(0x11, 0, 5);
        let hash = pool.add_raw_transaction(raw.clone()).unwrap();
        assert_eq!(hash, keccak256(&raw));
        assert_eq!(pool.stats().pooled, 1);
        assert_eq!(pool.stats().raw_buffered, 1);
    }

    #[test]
    fn orders_distinct_senders_by_tip_desc() {
        let mut pool = Mempool::new(0, 0);
        let low = pool.add_raw_transaction(raw_tx(0x11, 0, 1)).unwrap();
        let high = pool.add_raw_transaction(raw_tx(0x22, 0, 100)).unwrap();
        let mid = pool.add_raw_transaction(raw_tx(0x33, 0, 50)).unwrap();
        assert_eq!(pool.best_transaction_hashes(), vec![high, mid, low]);
    }

    #[test]
    fn respects_per_sender_nonce_order_regardless_of_tip() {
        // Same sender: nonce 0 must precede nonce 1 even though nonce 1 tips more.
        let mut pool = Mempool::new(0, 0);
        let n0 = pool.add_raw_transaction(raw_tx(0x11, 0, 1)).unwrap();
        let n1 = pool.add_raw_transaction(raw_tx(0x11, 1, 999)).unwrap();
        assert_eq!(pool.best_transaction_hashes(), vec![n0, n1]);
    }

    #[test]
    fn drain_empties_raw_buffer_but_keeps_pool() {
        let mut pool = Mempool::new(0, 0);
        pool.add_raw_transaction(raw_tx(0x11, 0, 1)).unwrap();
        assert_eq!(pool.drain_raw_transactions().len(), 1);
        assert_eq!(pool.drain_raw_transactions().len(), 0);
        // ordered set is unaffected by draining the raw buffer
        assert_eq!(pool.best_transaction_hashes().len(), 1);
    }

    #[test]
    fn clears_ordered_set_every_n_calls() {
        let mut pool = Mempool::new(0, 3);
        pool.add_raw_transaction(raw_tx(0x11, 0, 1)).unwrap();
        assert_eq!(pool.best_transaction_hashes().len(), 1); // call 1
        assert_eq!(pool.best_transaction_hashes().len(), 1); // call 2
        assert_eq!(pool.best_transaction_hashes().len(), 1); // call 3 -> clears after
        assert_eq!(pool.best_transaction_hashes().len(), 0); // now empty
    }

    #[test]
    fn dedups_resent_transaction() {
        let mut pool = Mempool::new(0, 0);
        let raw = raw_tx(0x11, 0, 1);
        let h1 = pool.add_raw_transaction(raw.clone()).unwrap();
        let h2 = pool.add_raw_transaction(raw).unwrap();
        assert_eq!(h1, h2);
        assert_eq!(pool.stats().pooled, 1); // pool dedups
        assert_eq!(pool.stats().raw_buffered, 2); // raw buffer keeps both
    }
}
