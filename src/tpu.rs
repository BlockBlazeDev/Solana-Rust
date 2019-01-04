//! The `tpu` module implements the Transaction Processing Unit, a
//! 5-stage transaction processing pipeline in software.

use crate::bank::Bank;
use crate::banking_stage::{BankingStage, BankingStageReturnType};
use crate::entry::Entry;
use crate::fetch_stage::FetchStage;
use crate::poh_service::Config;
use crate::service::Service;
use crate::sigverify_stage::SigVerifyStage;
use solana_sdk::hash::Hash;
use solana_sdk::pubkey::Pubkey;
use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::thread;

pub enum TpuReturnType {
    LeaderRotation,
}

pub struct Tpu {
    fetch_stage: FetchStage,
    sigverify_stage: SigVerifyStage,
    banking_stage: BankingStage,
    exit: Arc<AtomicBool>,
}

impl Tpu {
    #[allow(clippy::new_ret_no_self)]
    pub fn new(
        bank: &Arc<Bank>,
        tick_duration: Config,
        transactions_sockets: Vec<UdpSocket>,
        sigverify_disabled: bool,
        max_tick_height: Option<u64>,
        last_entry_id: &Hash,
        leader_id: Pubkey,
    ) -> (Self, Receiver<Vec<Entry>>, Arc<AtomicBool>) {
        let exit = Arc::new(AtomicBool::new(false));

        let (fetch_stage, packet_receiver) = FetchStage::new(transactions_sockets, exit.clone());

        let (sigverify_stage, verified_receiver) =
            SigVerifyStage::new(packet_receiver, sigverify_disabled);

        let (banking_stage, entry_receiver) = BankingStage::new(
            &bank,
            verified_receiver,
            tick_duration,
            last_entry_id,
            max_tick_height,
            leader_id,
        );

        let tpu = Self {
            fetch_stage,
            sigverify_stage,
            banking_stage,
            exit: exit.clone(),
        };

        (tpu, entry_receiver, exit)
    }

    pub fn exit(&self) {
        self.exit.store(true, Ordering::Relaxed);
    }

    pub fn is_exited(&self) -> bool {
        self.exit.load(Ordering::Relaxed)
    }

    pub fn close(self) -> thread::Result<Option<TpuReturnType>> {
        self.fetch_stage.close();
        self.join()
    }
}

impl Service for Tpu {
    type JoinReturnType = Option<TpuReturnType>;

    fn join(self) -> thread::Result<(Option<TpuReturnType>)> {
        self.fetch_stage.join()?;
        self.sigverify_stage.join()?;
        match self.banking_stage.join()? {
            Some(BankingStageReturnType::LeaderRotation) => Ok(Some(TpuReturnType::LeaderRotation)),
            _ => Ok(None),
        }
    }
}
