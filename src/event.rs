//! The `log` crate provides the foundational data structures for Proof-of-History,
//! an ordered log of events in time.

/// Each log entry contains three pieces of data. The 'num_hashes' field is the number
/// of hashes performed since the previous entry.  The 'end_hash' field is the result
/// of hashing 'end_hash' from the previous entry 'num_hashes' times.  The 'event'
/// field points to an Event that took place shortly after 'end_hash' was generated.
///
/// If you divide 'num_hashes' by the amount of time it takes to generate a new hash, you
/// get a duration estimate since the last event. Since processing power increases
/// over time, one should expect the duration 'num_hashes' represents to decrease proportionally.
/// Though processing power varies across nodes, the network gives priority to the
/// fastest processor. Duration should therefore be estimated by assuming that the hash
/// was generated by the fastest processor at the time the entry was logged.

use generic_array::GenericArray;
use generic_array::typenum::{U32, U64};
use ring::signature::Ed25519KeyPair;
use serde::Serialize;

pub type PublicKey = GenericArray<u8, U32>;
pub type Signature = GenericArray<u8, U64>;

/// When 'event' is Tick, the event represents a simple clock tick, and exists for the
/// sole purpose of improving the performance of event log verification. A tick can
/// be generated in 'num_hashes' hashes and verified in 'num_hashes' hashes.  By logging
/// a hash alongside the tick, each tick and be verified in parallel using the 'end_hash'
/// of the preceding tick to seed its hashing.
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub enum Event<T> {
    Tick,
    Claim {
        to: PublicKey,
        data: T,
        sig: Signature,
    },
    Transaction {
        from: Option<PublicKey>,
        to: PublicKey,
        data: T,
        sig: Signature,
    },
}

/// Return a new ED25519 keypair
pub fn generate_keypair() -> Ed25519KeyPair {
    use ring::{rand, signature};
    use untrusted;
    let rng = rand::SystemRandom::new();
    let pkcs8_bytes = signature::Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
    signature::Ed25519KeyPair::from_pkcs8(untrusted::Input::from(&pkcs8_bytes)).unwrap()
}

/// Return the public key for the given keypair
pub fn get_pubkey(keypair: &Ed25519KeyPair) -> PublicKey {
    GenericArray::clone_from_slice(keypair.public_key_bytes())
}

/// Return a signature for the given data using the private key from the given keypair.
pub fn sign_serialized<T: Serialize>(data: &T, keypair: &Ed25519KeyPair) -> Signature {
    use bincode::serialize;
    let serialized = serialize(data).unwrap();
    GenericArray::clone_from_slice(keypair.sign(&serialized).as_ref())
}

/// Return a signature for the given transaction data using the private key from the given keypair.
pub fn sign_transaction_data<T: Serialize>(
    data: &T,
    keypair: &Ed25519KeyPair,
    to: &PublicKey,
) -> Signature {
    sign_serialized(&(data, to), keypair)
}

/// Verify a signed message with the given public key.
pub fn verify_signature(peer_public_key_bytes: &[u8], msg_bytes: &[u8], sig_bytes: &[u8]) -> bool {
    use untrusted;
    use ring::signature;
    let peer_public_key = untrusted::Input::from(peer_public_key_bytes);
    let msg = untrusted::Input::from(msg_bytes);
    let sig = untrusted::Input::from(sig_bytes);
    signature::verify(&signature::ED25519, peer_public_key, msg, sig).is_ok()
}

pub fn get_signature<T>(event: &Event<T>) -> Option<Signature> {
    match *event {
        Event::Tick => None,
        Event::Claim { sig, .. } => Some(sig),
        Event::Transaction { sig, .. } => Some(sig),
    }
}

pub fn verify_event<T: Serialize>(event: &Event<T>) -> bool {
    use bincode::serialize;
    if let Event::Claim { to, ref data, sig } = *event {
        let mut claim_data = serialize(&data).unwrap();
        if !verify_signature(&to, &claim_data, &sig) {
            return false;
        }
    }
    if let Event::Transaction {
        from,
        to,
        ref data,
        sig,
    } = *event
    {
        let sign_data = serialize(&(&data, &to)).unwrap();
        if !verify_signature(&from.unwrap_or(to), &sign_data, &sig) {
            return false;
        }
    }
    true
}
