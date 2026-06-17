use std::cmp::Ordering;
use std::collections::{BTreeMap, BinaryHeap, HashMap};

use alloy_consensus::transaction::SignerRecoverable;
use alloy_consensus::{Transaction, TxEnvelope};
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
    /// Effective tip per gas at the configured base fee.
    effective_tip: u128,
    /// Insertion order — deterministic tie-break (first-seen wins).
    seq: u64,
}

/// In-enclave transaction pool + deterministic tip ordering.
///
/// # Nonce handling without chain state
///
/// The enclave never sees on-chain account state — it only has the stream of
/// submitted transactions. It does **not** need that state to *order* them:
///
///   * **Per-sender sequencing.** Within a sender, only the contiguous run of
///     nonces starting at an anchor is "ready"; anything past a gap is held
///     ("queued") until the gap fills. A `nonce 0, nonce 2` pair emits only
///     `0` until `1` arrives.
///   * **Replace-by-fee.** Two txs sharing `(sender, nonce)` collapse to the
///     higher-effective-tip one (ties: first-seen).
///   * **Cross-sender priority.** Ready heads are merged greedily by effective
///     tip.
///
/// The anchor is the lowest nonce seen for the sender, *unless* an on-chain
/// nonce is supplied via [`Mempool::set_account_nonce`] (an untrusted hint the
/// host can source from `eth_getTransactionCount`). That hint only refines
/// readiness — final validity is enforced by the builder when it executes the
/// ordering, so a wrong hint can at worst make the ordering suboptimal, never
/// unsafe. This is strictly more faithful than the old "fake the account nonce
/// per tx" trick, and needs no state-provider abstraction at all.
#[derive(Debug)]
pub struct Mempool {
    /// Base fee used to compute effective tips. The builder's pending base fee
    /// should be fed in here; 0 makes tip == priority fee.
    base_fee: u64,
    /// Optional per-sender on-chain nonce hints (anchors the ready run, drops
    /// stale txs below it). Untrusted; see the type docs.
    base_nonces: HashMap<Address, u64>,
    /// Drained wholesale by `get_raw_transactions` (legacy side buffer).
    raw_buffer: Vec<Bytes>,
    /// The working set, keyed by hash for dedup.
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
            base_nonces: HashMap::new(),
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

    /// Supply the on-chain nonce for `sender` (an untrusted hint, e.g. from the
    /// host's `eth_getTransactionCount`). Anchors that sender's ready run and
    /// drops any held txs below it. Optional — without it the lowest seen nonce
    /// is used as the anchor.
    pub fn set_account_nonce(&mut self, sender: Address, nonce: u64) {
        self.base_nonces.insert(sender, nonce);
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
    /// working set.
    pub fn add_raw_transaction(&mut self, raw: Bytes) -> Result<B256, MempoolError> {
        // The canonical tx hash is keccak256 of the EIP-2718 encoding, which is
        // precisely the bytes the submitter sent — hash them directly so we are
        // faithful even to non-canonical re-encodings.
        let hash = keccak256(&raw);

        let mut slice: &[u8] = raw.as_ref();
        let envelope =
            TxEnvelope::decode_2718(&mut slice).map_err(|e| MempoolError::Decode(e.to_string()))?;

        let sender = envelope
            .recover_signer()
            .map_err(|e| MempoolError::Recovery(e.to_string()))?;

        let effective_tip = envelope.effective_tip_per_gas(self.base_fee).unwrap_or(0);
        let nonce = envelope.nonce();

        let seq = self.next_seq;
        self.next_seq += 1;

        // Drain buffer keeps every submission in arrival order.
        self.raw_buffer.push(raw);

        // Working set dedups by hash (an identical resend doesn't reorder).
        self.entries.entry(hash).or_insert(PoolEntry { hash, sender, nonce, effective_tip, seq });

        Ok(hash)
    }

    /// `tog_getRawTransactions`: drain and return arrival-order raw bytes.
    pub fn drain_raw_transactions(&mut self) -> Vec<Bytes> {
        std::mem::take(&mut self.raw_buffer)
    }

    /// `tog_getBestTransactionHashes`: deterministic, gap-aware, tip-priority
    /// ordering. Clears the working set every `clear_every` calls (legacy
    /// behaviour).
    pub fn best_transaction_hashes(&mut self) -> Vec<B256> {
        let ordered = self.compute_order();
        self.best_calls += 1;
        if self.clear_every != 0 && self.best_calls.is_multiple_of(self.clear_every) {
            self.entries.clear();
        }
        ordered
    }

    /// Pure ordering computation (no side effects) — exposed for testing.
    fn compute_order(&self) -> Vec<B256> {
        // 1. Group by sender; collapse (sender, nonce) collisions replace-by-fee.
        let mut by_sender: HashMap<Address, BTreeMap<u64, &PoolEntry>> = HashMap::new();
        for e in self.entries.values() {
            let slot = by_sender.entry(e.sender).or_default();
            match slot.get(&e.nonce) {
                // Keep the incumbent only if it's strictly preferred.
                Some(existing) if preferred(existing, e) => {}
                _ => {
                    slot.insert(e.nonce, e);
                }
            }
        }

        // 2. Per sender, take the contiguous "ready" run from the anchor.
        let mut ready: HashMap<Address, Vec<&PoolEntry>> = HashMap::new();
        for (sender, by_nonce) in &by_sender {
            // Anchor = on-chain hint if known, else the lowest nonce we hold.
            let anchor = self
                .base_nonces
                .get(sender)
                .copied()
                .unwrap_or_else(|| *by_nonce.keys().next().expect("non-empty"));

            let mut expected = anchor;
            let mut run = Vec::new();
            // range(anchor..) skips stale txs below the anchor and yields the
            // rest in ascending nonce order.
            for (&nonce, &e) in by_nonce.range(anchor..) {
                if nonce == expected {
                    run.push(e);
                    expected += 1;
                } else {
                    break; // gap: everything from here on is queued
                }
            }
            if !run.is_empty() {
                ready.insert(*sender, run);
            }
        }

        // 3. Greedy tip-priority merge of the ready heads.
        let mut heads: HashMap<Address, usize> = ready.keys().map(|s| (*s, 0)).collect();
        let mut heap: BinaryHeap<HeapItem> = BinaryHeap::new();
        for (sender, run) in &ready {
            heap.push(HeapItem::new(run[0], *sender));
        }

        let mut out = Vec::with_capacity(self.entries.len());
        while let Some(item) = heap.pop() {
            out.push(item.hash);
            let run = &ready[&item.sender];
            let idx = heads.get_mut(&item.sender).expect("sender present");
            *idx += 1;
            if let Some(next) = run.get(*idx) {
                heap.push(HeapItem::new(next, item.sender));
            }
        }
        out
    }
}

/// Is `a` strictly preferred over `b` for the same `(sender, nonce)` slot?
/// Higher effective tip wins (replace-by-fee); ties go to the first-seen tx.
/// Deterministic regardless of map iteration order.
fn preferred(a: &PoolEntry, b: &PoolEntry) -> bool {
    a.effective_tip > b.effective_tip
        || (a.effective_tip == b.effective_tip && a.seq < b.seq)
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

    fn signer(key_byte: u8) -> PrivateKeySigner {
        PrivateKeySigner::from_bytes(&B256::repeat_byte(key_byte)).unwrap()
    }

    fn sender_of(key_byte: u8) -> Address {
        signer(key_byte).address()
    }

    /// Build a signed EIP-1559 tx as the raw 2718 bytes a client would submit.
    fn raw_tx(key_byte: u8, nonce: u64, priority_fee: u128) -> Bytes {
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
        let sig = signer(key_byte).sign_hash_sync(&tx.signature_hash()).unwrap();
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
    fn holds_transactions_after_a_nonce_gap() {
        let mut pool = Mempool::new(0, 0);
        let n0 = pool.add_raw_transaction(raw_tx(0x11, 0, 9)).unwrap();
        let _n2 = pool.add_raw_transaction(raw_tx(0x11, 2, 9)).unwrap(); // gap at 1
        // only nonce 0 is ready; nonce 2 is queued behind the gap
        assert_eq!(pool.best_transaction_hashes(), vec![n0]);

        // fill the gap -> all three become ready, in nonce order
        let n1 = pool.add_raw_transaction(raw_tx(0x11, 1, 9)).unwrap();
        let best = pool.best_transaction_hashes();
        assert_eq!(best.len(), 3);
        assert_eq!(best[0], n0);
        assert_eq!(best[1], n1);
    }

    #[test]
    fn replace_by_fee_keeps_higher_tip() {
        let mut pool = Mempool::new(0, 0);
        let cheap = pool.add_raw_transaction(raw_tx(0x11, 0, 1)).unwrap();
        let rich = pool.add_raw_transaction(raw_tx(0x11, 0, 100)).unwrap(); // same nonce
        assert_ne!(cheap, rich);
        assert_eq!(pool.stats().pooled, 2); // both stored
        // but only the higher-fee one is selected for the slot
        assert_eq!(pool.best_transaction_hashes(), vec![rich]);
    }

    #[test]
    fn account_nonce_hint_drops_stale_and_anchors() {
        let mut pool = Mempool::new(0, 0);
        // sender holds nonces 5 and 6
        let _n5 = pool.add_raw_transaction(raw_tx(0x11, 5, 9)).unwrap();
        let n6 = pool.add_raw_transaction(raw_tx(0x11, 6, 9)).unwrap();
        // on-chain nonce is 6 -> nonce 5 is stale, only 6 is ready
        pool.set_account_nonce(sender_of(0x11), 6);
        assert_eq!(pool.best_transaction_hashes(), vec![n6]);
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
