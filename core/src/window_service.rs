//! `window_service` handles the data plane incoming blobs, storing them in
//!   blocktree and retransmitting where required
//!
use crate::blocktree::Blocktree;
use crate::cluster_info::ClusterInfo;
use crate::packet::{SharedBlob, BLOB_HEADER_SIZE};
use crate::repair_service::{RepairService, RepairSlotRange};
use crate::result::{Error, Result};
use crate::service::Service;
use crate::streamer::{BlobReceiver, BlobSender};
use solana_metrics::counter::Counter;
use solana_metrics::{influxdb, submit};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::timing::duration_as_ms;
use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, RwLock};
use std::thread::{self, Builder, JoinHandle};
use std::time::{Duration, Instant};

fn retransmit_blobs(blobs: &[SharedBlob], retransmit: &BlobSender, id: &Pubkey) -> Result<()> {
    let mut retransmit_queue: Vec<SharedBlob> = Vec::new();
    for blob in blobs {
        // Don't add blobs generated by this node to the retransmit queue
        if blob.read().unwrap().id() != *id {
            let mut w_blob = blob.write().unwrap();
            w_blob.meta.forward = w_blob.should_forward();
            w_blob.set_forwarded(false);
            retransmit_queue.push(blob.clone());
        }
    }

    if !retransmit_queue.is_empty() {
        inc_new_counter_info!("streamer-recv_window-retransmit", retransmit_queue.len());
        retransmit.send(retransmit_queue)?;
    }
    Ok(())
}

/// Process a blob: Add blob to the ledger window.
fn process_blobs(blobs: &[SharedBlob], blocktree: &Arc<Blocktree>) -> Result<()> {
    // make an iterator for insert_data_blobs()
    let blobs: Vec<_> = blobs.iter().map(move |blob| blob.read().unwrap()).collect();

    blocktree.insert_data_blobs(blobs.iter().filter_map(|blob| {
        if !blob.is_coding() {
            Some(&(**blob))
        } else {
            None
        }
    }))?;

    for blob in blobs {
        // TODO: Once the original leader signature is added to the blob, make sure that
        // the blob was originally generated by the expected leader for this slot

        // Insert the new blob into block tree
        if blob.is_coding() {
            blocktree.put_coding_blob_bytes(
                blob.slot(),
                blob.index(),
                &blob.data[..BLOB_HEADER_SIZE + blob.size()],
            )?;
        }
    }
    Ok(())
}

fn recv_window(
    blocktree: &Arc<Blocktree>,
    id: &Pubkey,
    r: &BlobReceiver,
    retransmit: &BlobSender,
) -> Result<()> {
    let timer = Duration::from_millis(200);
    let mut dq = r.recv_timeout(timer)?;

    while let Ok(mut nq) = r.try_recv() {
        dq.append(&mut nq)
    }
    let now = Instant::now();
    inc_new_counter_info!("streamer-recv_window-recv", dq.len(), 100);

    submit(
        influxdb::Point::new("recv-window")
            .add_field("count", influxdb::Value::Integer(dq.len() as i64))
            .to_owned(),
    );

    retransmit_blobs(&dq, retransmit, id)?;

    //send a contiguous set of blocks
    trace!("{} num blobs received: {}", id, dq.len());

    process_blobs(&dq, blocktree)?;

    trace!(
        "Elapsed processing time in recv_window(): {}",
        duration_as_ms(&now.elapsed())
    );

    Ok(())
}

// Implement a destructor for the window_service thread to signal it exited
// even on panics
struct Finalizer {
    exit_sender: Arc<AtomicBool>,
}

impl Finalizer {
    fn new(exit_sender: Arc<AtomicBool>) -> Self {
        Finalizer { exit_sender }
    }
}
// Implement a destructor for Finalizer.
impl Drop for Finalizer {
    fn drop(&mut self) {
        self.exit_sender.clone().store(true, Ordering::Relaxed);
    }
}

pub struct WindowService {
    t_window: JoinHandle<()>,
    repair_service: RepairService,
}

impl WindowService {
    pub fn new(
        blocktree: Arc<Blocktree>,
        cluster_info: Arc<RwLock<ClusterInfo>>,
        r: BlobReceiver,
        retransmit: BlobSender,
        repair_socket: Arc<UdpSocket>,
        exit: &Arc<AtomicBool>,
        repair_slot_range: Option<RepairSlotRange>,
    ) -> WindowService {
        let repair_service = RepairService::new(
            blocktree.clone(),
            exit,
            repair_socket,
            cluster_info.clone(),
            repair_slot_range,
        );
        let exit = exit.clone();
        let t_window = Builder::new()
            .name("solana-window".to_string())
            .spawn(move || {
                let _exit = Finalizer::new(exit.clone());
                let id = cluster_info.read().unwrap().id();
                trace!("{}: RECV_WINDOW started", id);
                loop {
                    if exit.load(Ordering::Relaxed) {
                        break;
                    }
                    if let Err(e) = recv_window(&blocktree, &id, &r, &retransmit) {
                        match e {
                            Error::RecvTimeoutError(RecvTimeoutError::Disconnected) => break,
                            Error::RecvTimeoutError(RecvTimeoutError::Timeout) => (),
                            _ => {
                                inc_new_counter_info!("streamer-window-error", 1, 1);
                                error!("window error: {:?}", e);
                            }
                        }
                    }
                }
            })
            .unwrap();

        WindowService {
            t_window,
            repair_service,
        }
    }
}

impl Service for WindowService {
    type JoinReturnType = ();

    fn join(self) -> thread::Result<()> {
        self.t_window.join()?;
        self.repair_service.join()
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::blocktree::{get_tmp_ledger_path, Blocktree};
    use crate::cluster_info::{ClusterInfo, Node};
    use crate::entry::{make_consecutive_blobs, make_tiny_test_entries, EntrySlice};
    use crate::packet::index_blobs;
    use crate::service::Service;
    use crate::streamer::{blob_receiver, responder};
    use solana_sdk::hash::Hash;
    use std::fs::remove_dir_all;
    use std::net::UdpSocket;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::channel;
    use std::sync::{Arc, RwLock};
    use std::time::Duration;

    #[test]
    fn test_process_blob() {
        let blocktree_path = get_tmp_ledger_path!();
        let blocktree = Arc::new(Blocktree::open(&blocktree_path).unwrap());
        let num_entries = 10;
        let original_entries = make_tiny_test_entries(num_entries);
        let shared_blobs = original_entries.clone().to_shared_blobs();

        index_blobs(&shared_blobs, &Pubkey::new_rand(), 0, 0, 0);

        for blob in shared_blobs.into_iter().rev() {
            process_blobs(&[blob], &blocktree).expect("Expect successful processing of blob");
        }

        assert_eq!(
            blocktree.get_slot_entries(0, 0, None).unwrap(),
            original_entries
        );

        drop(blocktree);
        Blocktree::destroy(&blocktree_path).expect("Expected successful database destruction");
    }

    #[test]
    pub fn window_send_test() {
        solana_logger::setup();
        // setup a leader whose id is used to generates blobs and a validator
        // node whose window service will retransmit leader blobs.
        let leader_node = Node::new_localhost();
        let validator_node = Node::new_localhost();
        let exit = Arc::new(AtomicBool::new(false));
        let cluster_info_me = ClusterInfo::new_with_invalid_keypair(validator_node.info.clone());
        let me_id = leader_node.info.id;
        let subs = Arc::new(RwLock::new(cluster_info_me));

        let (s_reader, r_reader) = channel();
        let t_receiver = blob_receiver(Arc::new(leader_node.sockets.gossip), &exit, s_reader);
        let (s_retransmit, r_retransmit) = channel();
        let blocktree_path = get_tmp_ledger_path!();
        let blocktree = Arc::new(
            Blocktree::open(&blocktree_path).expect("Expected to be able to open database ledger"),
        );
        let t_window = WindowService::new(
            blocktree,
            subs,
            r_reader,
            s_retransmit,
            Arc::new(leader_node.sockets.repair),
            &exit,
            None,
        );
        let t_responder = {
            let (s_responder, r_responder) = channel();
            let blob_sockets: Vec<Arc<UdpSocket>> =
                leader_node.sockets.tvu.into_iter().map(Arc::new).collect();

            let t_responder = responder("window_send_test", blob_sockets[0].clone(), r_responder);
            let num_blobs_to_make = 10;
            let gossip_address = &leader_node.info.gossip;
            let msgs = make_consecutive_blobs(
                &me_id,
                num_blobs_to_make,
                0,
                Hash::default(),
                &gossip_address,
            )
            .into_iter()
            .rev()
            .collect();;
            s_responder.send(msgs).expect("send");
            t_responder
        };

        let max_attempts = 10;
        let mut num_attempts = 0;
        let mut q = Vec::new();
        loop {
            assert!(num_attempts != max_attempts);
            while let Ok(mut nq) = r_retransmit.recv_timeout(Duration::from_millis(500)) {
                q.append(&mut nq);
            }
            if q.len() == 10 {
                break;
            }
            num_attempts += 1;
        }

        exit.store(true, Ordering::Relaxed);
        t_receiver.join().expect("join");
        t_responder.join().expect("join");
        t_window.join().expect("join");
        Blocktree::destroy(&blocktree_path).expect("Expected successful database destruction");
        let _ignored = remove_dir_all(&blocktree_path);
    }

    #[test]
    pub fn window_send_leader_test2() {
        solana_logger::setup();
        // setup a leader whose id is used to generates blobs and a validator
        // node whose window service will retransmit leader blobs.
        let leader_node = Node::new_localhost();
        let validator_node = Node::new_localhost();
        let exit = Arc::new(AtomicBool::new(false));
        let cluster_info_me = ClusterInfo::new_with_invalid_keypair(validator_node.info.clone());
        let me_id = leader_node.info.id;
        let subs = Arc::new(RwLock::new(cluster_info_me));

        let (s_reader, r_reader) = channel();
        let t_receiver = blob_receiver(Arc::new(leader_node.sockets.gossip), &exit, s_reader);
        let (s_retransmit, r_retransmit) = channel();
        let blocktree_path = get_tmp_ledger_path!();
        let blocktree = Arc::new(
            Blocktree::open(&blocktree_path).expect("Expected to be able to open database ledger"),
        );
        let t_window = WindowService::new(
            blocktree,
            subs.clone(),
            r_reader,
            s_retransmit,
            Arc::new(leader_node.sockets.repair),
            &exit,
            None,
        );
        let t_responder = {
            let (s_responder, r_responder) = channel();
            let blob_sockets: Vec<Arc<UdpSocket>> =
                leader_node.sockets.tvu.into_iter().map(Arc::new).collect();
            let t_responder = responder("window_send_test", blob_sockets[0].clone(), r_responder);
            let mut msgs = Vec::new();
            let blobs =
                make_consecutive_blobs(&me_id, 14u64, 0, Hash::default(), &leader_node.info.gossip);

            for v in 0..10 {
                let i = 9 - v;
                msgs.push(blobs[i].clone());
            }
            s_responder.send(msgs).expect("send");

            let mut msgs1 = Vec::new();
            for v in 1..5 {
                let i = 9 + v;
                msgs1.push(blobs[i].clone());
            }
            s_responder.send(msgs1).expect("send");
            t_responder
        };
        let mut q = Vec::new();
        while let Ok(mut nq) = r_retransmit.recv_timeout(Duration::from_millis(500)) {
            q.append(&mut nq);
        }
        assert!(q.len() > 10);
        exit.store(true, Ordering::Relaxed);
        t_receiver.join().expect("join");
        t_responder.join().expect("join");
        t_window.join().expect("join");
        Blocktree::destroy(&blocktree_path).expect("Expected successful database destruction");
        let _ignored = remove_dir_all(&blocktree_path);
    }
}
