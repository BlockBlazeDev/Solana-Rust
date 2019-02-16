//! The `tpu` module implements the Transaction Processing Unit, a
//! multi-stage transaction processing pipeline in software.

use crate::bank::Bank;
use crate::banking_stage::{BankingStage, UnprocessedPackets};
use crate::blocktree::Blocktree;
use crate::broadcast_service::BroadcastService;
use crate::cluster_info::ClusterInfo;
use crate::cluster_info_vote_listener::ClusterInfoVoteListener;
use crate::fetch_stage::FetchStage;
use crate::leader_scheduler::LeaderScheduler;
use crate::poh_service::PohServiceConfig;
use crate::service::Service;
use crate::sigverify_stage::SigVerifyStage;
use crate::tpu_forwarder::TpuForwarder;
use solana_sdk::hash::Hash;
use solana_sdk::pubkey::Pubkey;
use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, RwLock};
use std::thread;

pub type TpuReturnType = u64; // tick_height to initiate a rotation
pub type TpuRotationSender = Sender<TpuReturnType>;
pub type TpuRotationReceiver = Receiver<TpuReturnType>;

pub enum TpuMode {
    Leader(LeaderServices),
    Forwarder(ForwarderServices),
}

pub struct LeaderServices {
    fetch_stage: Option<FetchStage>,
    sigverify_stage: Option<SigVerifyStage>,
    banking_stage: Option<BankingStage>,
    cluster_info_vote_listener: Option<ClusterInfoVoteListener>,
    broadcast_service: Option<BroadcastService>,
}

impl LeaderServices {
    fn new(
        fetch_stage: FetchStage,
        sigverify_stage: SigVerifyStage,
        banking_stage: BankingStage,
        cluster_info_vote_listener: ClusterInfoVoteListener,
        broadcast_service: BroadcastService,
    ) -> Self {
        LeaderServices {
            fetch_stage: Some(fetch_stage),
            sigverify_stage: Some(sigverify_stage),
            banking_stage: Some(banking_stage),
            cluster_info_vote_listener: Some(cluster_info_vote_listener),
            broadcast_service: Some(broadcast_service),
        }
    }

    fn exit(&self) {
        self.fetch_stage.as_ref().unwrap().close();
    }

    fn join(&mut self) -> thread::Result<()> {
        let mut results = vec![];
        results.push(self.fetch_stage.take().unwrap().join());
        results.push(self.sigverify_stage.take().unwrap().join());
        results.push(self.cluster_info_vote_listener.take().unwrap().join());
        results.push(self.banking_stage.take().unwrap().join());
        let broadcast_result = self.broadcast_service.take().unwrap().join();
        for result in results {
            result?;
        }
        let _ = broadcast_result?;
        Ok(())
    }

    fn close(&mut self) -> thread::Result<()> {
        self.exit();
        self.join()
    }
}

pub struct ForwarderServices {
    tpu_forwarder: Option<TpuForwarder>,
}

impl ForwarderServices {
    fn new(tpu_forwarder: TpuForwarder) -> Self {
        ForwarderServices {
            tpu_forwarder: Some(tpu_forwarder),
        }
    }

    fn exit(&self) {
        self.tpu_forwarder.as_ref().unwrap().close();
    }

    fn join(&mut self) -> thread::Result<()> {
        self.tpu_forwarder.take().unwrap().join()
    }

    fn close(&mut self) -> thread::Result<()> {
        self.exit();
        self.join()
    }
}

pub struct Tpu {
    tpu_mode: Option<TpuMode>,
    exit: Arc<AtomicBool>,
    id: Pubkey,
    cluster_info: Arc<RwLock<ClusterInfo>>,
}

impl Tpu {
    pub fn new(id: Pubkey, cluster_info: &Arc<RwLock<ClusterInfo>>) -> Self {
        Self {
            tpu_mode: None,
            exit: Arc::new(AtomicBool::new(false)),
            id,
            cluster_info: cluster_info.clone(),
        }
    }

    fn mode_exit(&mut self) {
        match &mut self.tpu_mode {
            Some(TpuMode::Leader(svcs)) => {
                svcs.exit();
            }
            Some(TpuMode::Forwarder(svcs)) => {
                svcs.exit();
            }
            None => (),
        }
    }

    fn mode_close(&mut self) {
        match &mut self.tpu_mode {
            Some(TpuMode::Leader(svcs)) => {
                let _ = svcs.close();
            }
            Some(TpuMode::Forwarder(svcs)) => {
                let _ = svcs.close();
            }
            None => (),
        }
    }

    fn forward_unprocessed_packets(
        tpu: &std::net::SocketAddr,
        unprocessed_packets: UnprocessedPackets,
    ) -> std::io::Result<()> {
        let socket = UdpSocket::bind("0.0.0.0:0")?;
        for (packets, start_index) in unprocessed_packets {
            let packets = packets.read().unwrap();
            for packet in packets.packets.iter().skip(start_index) {
                socket.send_to(&packet.data[..packet.meta.size], tpu)?;
            }
        }
        Ok(())
    }

    fn close_and_forward_unprocessed_packets(&mut self) {
        self.mode_exit();

        let unprocessed_packets = match self.tpu_mode.take().as_mut() {
            Some(TpuMode::Leader(svcs)) => svcs
                .banking_stage
                .as_mut()
                .unwrap()
                .join_and_collect_unprocessed_packets(),
            Some(TpuMode::Forwarder(svcs)) => svcs
                .tpu_forwarder
                .as_mut()
                .unwrap()
                .join_and_collect_unprocessed_packets(),
            None => vec![],
        };

        if !unprocessed_packets.is_empty() {
            let tpu = self.cluster_info.read().unwrap().leader_data().unwrap().tpu;
            info!("forwarding unprocessed packets to new leader at {:?}", tpu);
            Tpu::forward_unprocessed_packets(&tpu, unprocessed_packets).unwrap_or_else(|err| {
                warn!("Failed to forward unprocessed transactions: {:?}", err)
            });
        }

        self.mode_close();
    }

    pub fn switch_to_forwarder(&mut self, leader_id: Pubkey, transactions_sockets: Vec<UdpSocket>) {
        self.close_and_forward_unprocessed_packets();

        self.cluster_info.write().unwrap().set_leader(leader_id);

        let tpu_forwarder = TpuForwarder::new(transactions_sockets, self.cluster_info.clone());
        self.tpu_mode = Some(TpuMode::Forwarder(ForwarderServices::new(tpu_forwarder)));
    }

    #[allow(clippy::too_many_arguments)]
    pub fn switch_to_leader(
        &mut self,
        bank: &Arc<Bank>,
        tick_duration: PohServiceConfig,
        transactions_sockets: Vec<UdpSocket>,
        broadcast_socket: UdpSocket,
        sigverify_disabled: bool,
        max_tick_height: u64,
        blob_index: u64,
        last_entry_id: &Hash,
        to_validator_sender: &TpuRotationSender,
        blocktree: &Arc<Blocktree>,
        leader_scheduler: &Arc<RwLock<LeaderScheduler>>,
    ) {
        self.close_and_forward_unprocessed_packets();

        self.cluster_info.write().unwrap().set_leader(self.id);

        self.exit = Arc::new(AtomicBool::new(false));
        let (packet_sender, packet_receiver) = channel();
        let fetch_stage = FetchStage::new_with_sender(
            transactions_sockets,
            self.exit.clone(),
            &packet_sender.clone(),
        );
        let cluster_info_vote_listener = ClusterInfoVoteListener::new(
            self.exit.clone(),
            self.cluster_info.clone(),
            packet_sender,
        );

        let (sigverify_stage, verified_receiver) =
            SigVerifyStage::new(packet_receiver, sigverify_disabled);

        let (banking_stage, entry_receiver) = BankingStage::new(
            &bank,
            verified_receiver,
            tick_duration,
            last_entry_id,
            max_tick_height,
            self.id,
            &to_validator_sender,
        );

        let broadcast_service = BroadcastService::new(
            bank.clone(),
            broadcast_socket,
            self.cluster_info.clone(),
            blob_index,
            leader_scheduler.clone(),
            entry_receiver,
            max_tick_height,
            self.exit.clone(),
            blocktree,
        );

        let svcs = LeaderServices::new(
            fetch_stage,
            sigverify_stage,
            banking_stage,
            cluster_info_vote_listener,
            broadcast_service,
        );
        self.tpu_mode = Some(TpuMode::Leader(svcs));
    }

    pub fn is_leader(&self) -> Option<bool> {
        match self.tpu_mode {
            Some(TpuMode::Leader(_)) => Some(true),
            Some(TpuMode::Forwarder(_)) => Some(false),
            None => None,
        }
    }

    pub fn exit(&self) {
        self.exit.store(true, Ordering::Relaxed);
    }

    pub fn is_exited(&self) -> bool {
        self.exit.load(Ordering::Relaxed)
    }

    pub fn close(mut self) -> thread::Result<()> {
        self.mode_close();
        self.join()
    }
}

impl Service for Tpu {
    type JoinReturnType = ();

    fn join(self) -> thread::Result<()> {
        match self.tpu_mode {
            Some(TpuMode::Leader(mut svcs)) => svcs.join()?,
            Some(TpuMode::Forwarder(mut svcs)) => svcs.join()?,
            None => (),
        }
        Ok(())
    }
}
