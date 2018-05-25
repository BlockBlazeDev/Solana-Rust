//! The `entry` module is a fundamental building block of Proof of History. It contains a
//! unique ID that is the hash of the Entry before it, plus the hash of the
//! transactions within it. Entries cannot be reordered, and its field `num_hashes`
//! represents an approximate amount of time since the last Entry was created.
use hash::{extend_and_hash, hash, Hash};
use rayon::prelude::*;
use transaction::Transaction;

/// Each Entry contains three pieces of data. The `num_hashes` field is the number
/// of hashes performed since the previous entry.  The `id` field is the result
/// of hashing `id` from the previous entry `num_hashes` times.  The `transactions`
/// field points to Events that took place shortly after `id` was generated.
///
/// If you divide `num_hashes` by the amount of time it takes to generate a new hash, you
/// get a duration estimate since the last Entry. Since processing power increases
/// over time, one should expect the duration `num_hashes` represents to decrease proportionally.
/// Though processing power varies across nodes, the network gives priority to the
/// fastest processor. Duration should therefore be estimated by assuming that the hash
/// was generated by the fastest processor at the time the entry was recorded.
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct Entry {
    pub num_hashes: u64,
    pub id: Hash,
    pub transactions: Vec<Transaction>,
}

impl Entry {
    /// Creates the next Entry `num_hashes` after `start_hash`.
    pub fn new(start_hash: &Hash, cur_hashes: u64, transactions: Vec<Transaction>) -> Self {
        let num_hashes = cur_hashes + if transactions.is_empty() { 0 } else { 1 };
        let id = next_hash(start_hash, 0, &transactions);
        Entry {
            num_hashes,
            id,
            transactions,
        }
    }

    /// Creates the next Tick Entry `num_hashes` after `start_hash`.
    pub fn new_mut(
        start_hash: &mut Hash,
        cur_hashes: &mut u64,
        transactions: Vec<Transaction>,
    ) -> Self {
        let entry = Self::new(start_hash, *cur_hashes, transactions);
        *start_hash = entry.id;
        *cur_hashes = 0;
        entry
    }

    /// Creates a Entry from the number of hashes `num_hashes` since the previous transaction
    /// and that resulting `id`.
    pub fn new_tick(num_hashes: u64, id: &Hash) -> Self {
        Entry {
            num_hashes,
            id: *id,
            transactions: vec![],
        }
    }

    /// Verifies self.id is the result of hashing a `start_hash` `self.num_hashes` times.
    /// If the transaction is not a Tick, then hash that as well.
    pub fn verify(&self, start_hash: &Hash) -> bool {
        self.transactions.par_iter().all(|tx| tx.verify_plan())
            && self.id == next_hash(start_hash, self.num_hashes, &self.transactions)
    }
}

fn add_transaction_data(hash_data: &mut Vec<u8>, tr: &Transaction) {
    hash_data.push(0u8);
    hash_data.extend_from_slice(&tr.sig);
}

/// Creates the hash `num_hashes` after `start_hash`. If the transaction contains
/// a signature, the final hash will be a hash of both the previous ID and
/// the signature.
pub fn next_hash(start_hash: &Hash, num_hashes: u64, transactions: &[Transaction]) -> Hash {
    let mut id = *start_hash;
    for _ in 1..num_hashes {
        id = hash(&id);
    }

    // Hash all the transaction data
    let mut hash_data = vec![];
    for tx in transactions {
        add_transaction_data(&mut hash_data, tx);
    }

    if !hash_data.is_empty() {
        extend_and_hash(&id, &hash_data)
    } else if num_hashes != 0 {
        hash(&id)
    } else {
        id
    }
}

/// Creates the next Tick or Event Entry `num_hashes` after `start_hash`.
pub fn next_entry(start_hash: &Hash, num_hashes: u64, transactions: Vec<Transaction>) -> Entry {
    Entry {
        num_hashes,
        id: next_hash(start_hash, num_hashes, &transactions),
        transactions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::prelude::*;
    use entry::Entry;
    use hash::hash;
    use signature::{KeyPair, KeyPairUtil};
    use transaction::Transaction;

    #[test]
    fn test_entry_verify() {
        let zero = Hash::default();
        let one = hash(&zero);
        assert!(Entry::new_tick(0, &zero).verify(&zero)); // base case
        assert!(!Entry::new_tick(0, &zero).verify(&one)); // base case, bad
        assert!(next_entry(&zero, 1, vec![]).verify(&zero)); // inductive step
        assert!(!next_entry(&zero, 1, vec![]).verify(&one)); // inductive step, bad
    }

    #[test]
    fn test_transaction_reorder_attack() {
        let zero = Hash::default();

        // First, verify entries
        let keypair = KeyPair::new();
        let tr0 = Transaction::new(&keypair, keypair.pubkey(), 0, zero);
        let tr1 = Transaction::new(&keypair, keypair.pubkey(), 1, zero);
        let mut e0 = Entry::new(&zero, 0, vec![tr0.clone(), tr1.clone()]);
        assert!(e0.verify(&zero));

        // Next, swap two transactions and ensure verification fails.
        e0.transactions[0] = tr1; // <-- attack
        e0.transactions[1] = tr0;
        assert!(!e0.verify(&zero));
    }

    #[test]
    fn test_witness_reorder_attack() {
        let zero = Hash::default();

        // First, verify entries
        let keypair = KeyPair::new();
        let tr0 = Transaction::new_timestamp(&keypair, Utc::now(), zero);
        let tr1 = Transaction::new_signature(&keypair, Default::default(), zero);
        let mut e0 = Entry::new(&zero, 0, vec![tr0.clone(), tr1.clone()]);
        assert!(e0.verify(&zero));

        // Next, swap two witness transactions and ensure verification fails.
        e0.transactions[0] = tr1; // <-- attack
        e0.transactions[1] = tr0;
        assert!(!e0.verify(&zero));
    }

    #[test]
    fn test_next_entry() {
        let zero = Hash::default();
        let tick = next_entry(&zero, 1, vec![]);
        assert_eq!(tick.num_hashes, 1);
        assert_ne!(tick.id, zero);
    }
}
