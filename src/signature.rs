//! The `signature` module provides functionality for public, and private keys.

use bs58;
use generic_array::typenum::{U32, U64};
use generic_array::GenericArray;
use rand::{ChaChaRng, Rng, SeedableRng};
use rayon::prelude::*;
use ring::signature::Ed25519KeyPair;
use ring::{rand, signature};
use serde_json;
use std::error;
use std::fmt;
use std::fs::File;
use untrusted::Input;

pub type KeyPair = Ed25519KeyPair;
#[derive(Serialize, Deserialize, Clone, Copy, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct PublicKey(GenericArray<u8, U32>);

impl PublicKey {
    pub fn new(pubkey_vec: &[u8]) -> Self {
        PublicKey(GenericArray::clone_from_slice(&pubkey_vec))
    }
}

impl AsRef<[u8]> for PublicKey {
    fn as_ref(&self) -> &[u8] {
        &self.0[..]
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", bs58::encode(self.0).into_string())
    }
}

impl fmt::Display for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", bs58::encode(self.0).into_string())
    }
}

pub type Signature = GenericArray<u8, U64>;

pub trait KeyPairUtil {
    fn new() -> Self;
    fn pubkey(&self) -> PublicKey;
}

impl KeyPairUtil for Ed25519KeyPair {
    /// Return a new ED25519 keypair
    fn new() -> Self {
        let rng = rand::SystemRandom::new();
        let pkcs8_bytes = Ed25519KeyPair::generate_pkcs8(&rng).expect("generate_pkcs8");
        Ed25519KeyPair::from_pkcs8(Input::from(&pkcs8_bytes)).expect("from_pcks8")
    }

    /// Return the public key for the given keypair
    fn pubkey(&self) -> PublicKey {
        PublicKey(GenericArray::clone_from_slice(self.public_key_bytes()))
    }
}

pub trait SignatureUtil {
    fn verify(&self, peer_public_key_bytes: &[u8], msg_bytes: &[u8]) -> bool;
}

impl SignatureUtil for GenericArray<u8, U64> {
    fn verify(&self, peer_public_key_bytes: &[u8], msg_bytes: &[u8]) -> bool {
        let peer_public_key = Input::from(peer_public_key_bytes);
        let msg = Input::from(msg_bytes);
        let sig = Input::from(self);
        signature::verify(&signature::ED25519, peer_public_key, msg, sig).is_ok()
    }
}

pub struct GenKeys {
    generator: ChaChaRng,
}

impl GenKeys {
    pub fn new(seed: [u8; 32]) -> GenKeys {
        let generator = ChaChaRng::from_seed(seed);
        GenKeys { generator }
    }

    fn gen_seed(&mut self) -> [u8; 32] {
        let mut seed = [0u8; 32];
        self.generator.fill(&mut seed);
        seed
    }

    fn gen_n_seeds(&mut self, n: i64) -> Vec<[u8; 32]> {
        (0..n).map(|_| self.gen_seed()).collect()
    }

    pub fn gen_n_keypairs(&mut self, n: i64) -> Vec<KeyPair> {
        self.gen_n_seeds(n)
            .into_par_iter()
            .map(|seed| KeyPair::from_seed_unchecked(Input::from(&seed)).unwrap())
            .collect()
    }
}

pub fn read_pkcs8(path: &str) -> Result<Vec<u8>, Box<error::Error>> {
    let file = File::open(path.to_string())?;
    let pkcs8: Vec<u8> = serde_json::from_reader(file)?;
    Ok(pkcs8)
}

pub fn read_keypair(path: &str) -> Result<KeyPair, Box<error::Error>> {
    let pkcs8 = read_pkcs8(path)?;
    let keypair = Ed25519KeyPair::from_pkcs8(Input::from(&pkcs8))?;
    Ok(keypair)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn test_new_key_is_deterministic() {
        let seed = [0u8; 32];
        let mut gen0 = GenKeys::new(seed);
        let mut gen1 = GenKeys::new(seed);

        for _ in 0..100 {
            assert_eq!(gen0.gen_seed().to_vec(), gen1.gen_seed().to_vec());
        }
    }

    fn gen_n_pubkeys(seed: [u8; 32], n: i64) -> HashSet<PublicKey> {
        GenKeys::new(seed)
            .gen_n_keypairs(n)
            .into_iter()
            .map(|x| x.pubkey())
            .collect()
    }

    #[test]
    fn test_gen_n_pubkeys_deterministic() {
        let seed = [0u8; 32];
        assert_eq!(gen_n_pubkeys(seed, 50), gen_n_pubkeys(seed, 50));
    }
}
