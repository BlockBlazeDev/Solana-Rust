//! The `sigverify` module provides digital signature verification functions.
//! By default, signatures are verified in parallel using all available CPU
//! cores.  When perf-libs are available signature verification is offloaded
//! to the GPU.
//!

pub use solana_perf::sigverify::{
    batch_size, ed25519_verify_cpu, ed25519_verify_disabled, init, TxOffset,
};
use {
    crate::sigverify_stage::SigVerifier,
    solana_perf::{cuda_runtime::PinnedVec, packet::Packets, recycler::Recycler, sigverify},
};

#[derive(Clone)]
pub struct TransactionSigVerifier {
    recycler: Recycler<TxOffset>,
    recycler_out: Recycler<PinnedVec<u8>>,
    reject_non_vote: bool,
}

impl TransactionSigVerifier {
    pub fn new_reject_non_vote() -> Self {
        TransactionSigVerifier {
            reject_non_vote: true,
            ..TransactionSigVerifier::default()
        }
    }
}

impl Default for TransactionSigVerifier {
    fn default() -> Self {
        init();
        Self {
            recycler: Recycler::warmed(50, 4096),
            recycler_out: Recycler::warmed(50, 4096),
            reject_non_vote: false,
        }
    }
}

impl SigVerifier for TransactionSigVerifier {
    fn verify_batch(&self, mut batch: Vec<Packets>) -> Vec<Packets> {
        sigverify::ed25519_verify(
            &mut batch,
            &self.recycler,
            &self.recycler_out,
            self.reject_non_vote,
        );
        batch
    }
}
