//! Set of functions for emulating windowing functions from a database ledger implementation
use crate::blocktree::*;
use crate::counter::Counter;
#[cfg(feature = "erasure")]
use crate::erasure;
use crate::leader_scheduler::LeaderScheduler;
use crate::packet::{SharedBlob, BLOB_HEADER_SIZE};
use crate::result::Result;
use crate::streamer::BlobSender;
use log::Level;
use solana_metrics::{influxdb, submit};
use solana_sdk::pubkey::Pubkey;
use std::borrow::Borrow;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, RwLock};

pub const MAX_REPAIR_LENGTH: usize = 128;

pub fn retransmit_all_leader_blocks(
    dq: &[SharedBlob],
    leader_scheduler: &Arc<RwLock<LeaderScheduler>>,
    retransmit: &BlobSender,
    id: &Pubkey,
) -> Result<()> {
    let mut retransmit_queue: Vec<SharedBlob> = Vec::new();
    for b in dq {
        // Check if the blob is from the scheduled leader for its slot. If so,
        // add to the retransmit_queue
        let slot = b.read().unwrap().slot();
        if let Some(leader_id) = leader_scheduler.read().unwrap().get_leader_for_slot(slot) {
            if leader_id != *id {
                add_blob_to_retransmit_queue(b, leader_id, &mut retransmit_queue);
            }
        }
    }

    submit(
        influxdb::Point::new("retransmit-queue")
            .add_field(
                "count",
                influxdb::Value::Integer(retransmit_queue.len() as i64),
            )
            .to_owned(),
    );

    if !retransmit_queue.is_empty() {
        inc_new_counter_info!("streamer-recv_window-retransmit", retransmit_queue.len());
        retransmit.send(retransmit_queue)?;
    }
    Ok(())
}

pub fn add_blob_to_retransmit_queue(
    b: &SharedBlob,
    leader_id: Pubkey,
    retransmit_queue: &mut Vec<SharedBlob>,
) {
    let p = b.read().unwrap();
    if p.id() == leader_id {
        let nv = SharedBlob::default();
        {
            let mut mnv = nv.write().unwrap();
            let sz = p.meta.size;
            mnv.meta.size = sz;
            mnv.data[..sz].copy_from_slice(&p.data[..sz]);
        }
        retransmit_queue.push(nv);
    }
}

/// Process a blob: Add blob to the ledger window.
pub fn process_blob(
    leader_scheduler: &Arc<RwLock<LeaderScheduler>>,
    blocktree: &Arc<Blocktree>,
    blob: &SharedBlob,
) -> Result<()> {
    let is_coding = blob.read().unwrap().is_coding();

    // Check if the blob is in the range of our known leaders. If not, we return.
    let (slot, pix) = {
        let r_blob = blob.read().unwrap();
        (r_blob.slot(), r_blob.index())
    };
    let leader = leader_scheduler.read().unwrap().get_leader_for_slot(slot);

    // TODO: Once the original leader signature is added to the blob, make sure that
    // the blob was originally generated by the expected leader for this slot
    if leader.is_none() {
        warn!("No leader for slot {}, blob dropped", slot);
        return Ok(()); // Occurs as a leader is rotating into a validator
    }

    // Insert the new blob into block tree
    if is_coding {
        let blob = &blob.read().unwrap();
        blocktree.put_coding_blob_bytes(slot, pix, &blob.data[..BLOB_HEADER_SIZE + blob.size()])?;
    } else {
        blocktree.insert_data_blobs(vec![(*blob.read().unwrap()).borrow()])?;
    }

    #[cfg(feature = "erasure")]
    {
        // TODO: Support per-slot erasure. Issue: https://github.com/solana-labs/solana/issues/2441
        if let Err(e) = try_erasure(blocktree, 0) {
            trace!(
                "erasure::recover failed to write recovered coding blobs. Err: {:?}",
                e
            );
        }
    }

    Ok(())
}

#[cfg(feature = "erasure")]
fn try_erasure(blocktree: &Arc<Blocktree>, slot_index: u64) -> Result<()> {
    let meta = blocktree.meta(slot_index)?;

    if let Some(meta) = meta {
        let (data, coding) = erasure::recover(blocktree, slot_index, meta.consumed)?;
        for c in coding {
            let c = c.read().unwrap();
            blocktree.put_coding_blob_bytes(
                0,
                c.index(),
                &c.data[..BLOB_HEADER_SIZE + c.size()],
            )?;
        }

        blocktree.write_shared_blobs(data)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::blocktree::get_tmp_ledger_path;
    #[cfg(all(feature = "erasure", test))]
    use crate::entry::reconstruct_entries_from_blobs;
    use crate::entry::{make_tiny_test_entries, EntrySlice};
    #[cfg(all(feature = "erasure", test))]
    use crate::erasure::test::{generate_blocktree_from_window, setup_window_ledger};
    #[cfg(all(feature = "erasure", test))]
    use crate::erasure::{NUM_CODING, NUM_DATA};
    use crate::packet::{index_blobs, Blob, Packet, Packets, SharedBlob, PACKET_DATA_SIZE};
    use crate::streamer::{receiver, responder, PacketReceiver};
    use solana_sdk::signature::{Keypair, KeypairUtil};
    use std::io;
    use std::io::Write;
    use std::net::UdpSocket;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::channel;
    use std::sync::{Arc, RwLock};
    use std::time::Duration;

    fn get_msgs(r: PacketReceiver, num: &mut usize) {
        for _t in 0..5 {
            let timer = Duration::new(1, 0);
            match r.recv_timeout(timer) {
                Ok(m) => *num += m.read().unwrap().packets.len(),
                e => info!("error {:?}", e),
            }
            if *num == 10 {
                break;
            }
        }
    }
    #[test]
    pub fn streamer_debug() {
        write!(io::sink(), "{:?}", Packet::default()).unwrap();
        write!(io::sink(), "{:?}", Packets::default()).unwrap();
        write!(io::sink(), "{:?}", Blob::default()).unwrap();
    }

    #[test]
    pub fn streamer_send_test() {
        let read = UdpSocket::bind("127.0.0.1:0").expect("bind");
        read.set_read_timeout(Some(Duration::new(1, 0))).unwrap();

        let addr = read.local_addr().unwrap();
        let send = UdpSocket::bind("127.0.0.1:0").expect("bind");
        let exit = Arc::new(AtomicBool::new(false));
        let (s_reader, r_reader) = channel();
        let t_receiver = receiver(
            Arc::new(read),
            exit.clone(),
            s_reader,
            "window-streamer-test",
        );
        let t_responder = {
            let (s_responder, r_responder) = channel();
            let t_responder = responder("streamer_send_test", Arc::new(send), r_responder);
            let mut msgs = Vec::new();
            for i in 0..10 {
                let b = SharedBlob::default();
                {
                    let mut w = b.write().unwrap();
                    w.data[0] = i as u8;
                    w.meta.size = PACKET_DATA_SIZE;
                    w.meta.set_addr(&addr);
                }
                msgs.push(b);
            }
            s_responder.send(msgs).expect("send");
            t_responder
        };

        let mut num = 0;
        get_msgs(r_reader, &mut num);
        assert_eq!(num, 10);
        exit.store(true, Ordering::Relaxed);
        t_receiver.join().expect("join");
        t_responder.join().expect("join");
    }

    #[test]
    pub fn test_retransmit() {
        let leader = Keypair::new().pubkey();
        let nonleader = Keypair::new().pubkey();
        let mut leader_scheduler = LeaderScheduler::default();
        leader_scheduler.set_leader_schedule(vec![leader]);
        let leader_scheduler = Arc::new(RwLock::new(leader_scheduler));
        let blob = SharedBlob::default();

        let (blob_sender, blob_receiver) = channel();

        // Expect blob from leader to be retransmitted
        blob.write().unwrap().set_id(&leader);
        retransmit_all_leader_blocks(
            &vec![blob.clone()],
            &leader_scheduler,
            &blob_sender,
            &nonleader,
        )
        .expect("Expect successful retransmit");
        let output_blob = blob_receiver
            .try_recv()
            .expect("Expect input blob to be retransmitted");

        // Retransmitted blob should be missing the leader id
        assert_ne!(*output_blob[0].read().unwrap(), *blob.read().unwrap());
        // Set the leader in the retransmitted blob, should now match the original
        output_blob[0].write().unwrap().set_id(&leader);
        assert_eq!(*output_blob[0].read().unwrap(), *blob.read().unwrap());

        // Expect blob from nonleader to not be retransmitted
        blob.write().unwrap().set_id(&nonleader);
        retransmit_all_leader_blocks(
            &vec![blob.clone()],
            &leader_scheduler,
            &blob_sender,
            &nonleader,
        )
        .expect("Expect successful retransmit");
        assert!(blob_receiver.try_recv().is_err());

        // Expect blob from leader while currently leader to not be retransmitted
        blob.write().unwrap().set_id(&leader);
        retransmit_all_leader_blocks(&vec![blob], &leader_scheduler, &blob_sender, &leader)
            .expect("Expect successful retransmit");
        assert!(blob_receiver.try_recv().is_err());
    }

    #[test]
    pub fn test_find_missing_data_indexes_sanity() {
        let slot = DEFAULT_SLOT_HEIGHT;

        let blocktree_path = get_tmp_ledger_path("test_find_missing_data_indexes_sanity");
        let blocktree = Blocktree::open(&blocktree_path).unwrap();

        // Early exit conditions
        let empty: Vec<u64> = vec![];
        assert_eq!(blocktree.find_missing_data_indexes(slot, 0, 0, 1), empty);
        assert_eq!(blocktree.find_missing_data_indexes(slot, 5, 5, 1), empty);
        assert_eq!(blocktree.find_missing_data_indexes(slot, 4, 3, 1), empty);
        assert_eq!(blocktree.find_missing_data_indexes(slot, 1, 2, 0), empty);

        let mut blobs = make_tiny_test_entries(2).to_blobs();

        const ONE: u64 = 1;
        const OTHER: u64 = 4;

        blobs[0].set_index(ONE);
        blobs[1].set_index(OTHER);

        // Insert one blob at index = first_index
        blocktree.write_blobs(&blobs).unwrap();

        const STARTS: u64 = OTHER * 2;
        const END: u64 = OTHER * 3;
        const MAX: usize = 10;
        // The first blob has index = first_index. Thus, for i < first_index,
        // given the input range of [i, first_index], the missing indexes should be
        // [i, first_index - 1]
        for start in 0..STARTS {
            let result = blocktree.find_missing_data_indexes(
                slot, start, // start
                END,   //end
                MAX,   //max
            );
            let expected: Vec<u64> = (start..END).filter(|i| *i != ONE && *i != OTHER).collect();
            assert_eq!(result, expected);
        }

        drop(blocktree);
        Blocktree::destroy(&blocktree_path).expect("Expected successful database destruction");
    }

    #[test]
    pub fn test_find_missing_data_indexes() {
        let slot = DEFAULT_SLOT_HEIGHT;
        let blocktree_path = get_tmp_ledger_path("test_find_missing_data_indexes");
        let blocktree = Blocktree::open(&blocktree_path).unwrap();

        // Write entries
        let gap = 10;
        assert!(gap > 3);
        let num_entries = 10;
        let mut blobs = make_tiny_test_entries(num_entries).to_blobs();
        for (i, b) in blobs.iter_mut().enumerate() {
            b.set_index(i as u64 * gap);
            b.set_slot(slot);
        }
        blocktree.write_blobs(&blobs).unwrap();

        // Index of the first blob is 0
        // Index of the second blob is "gap"
        // Thus, the missing indexes should then be [1, gap - 1] for the input index
        // range of [0, gap)
        let expected: Vec<u64> = (1..gap).collect();
        assert_eq!(
            blocktree.find_missing_data_indexes(slot, 0, gap, gap as usize),
            expected
        );
        assert_eq!(
            blocktree.find_missing_data_indexes(slot, 1, gap, (gap - 1) as usize),
            expected,
        );
        assert_eq!(
            blocktree.find_missing_data_indexes(slot, 0, gap - 1, (gap - 1) as usize),
            &expected[..expected.len() - 1],
        );
        assert_eq!(
            blocktree.find_missing_data_indexes(slot, gap - 2, gap, gap as usize),
            vec![gap - 2, gap - 1],
        );
        assert_eq!(
            blocktree.find_missing_data_indexes(slot, gap - 2, gap, 1),
            vec![gap - 2],
        );
        assert_eq!(
            blocktree.find_missing_data_indexes(slot, 0, gap, 1),
            vec![1],
        );

        // Test with end indexes that are greater than the last item in the ledger
        let mut expected: Vec<u64> = (1..gap).collect();
        expected.push(gap + 1);
        assert_eq!(
            blocktree.find_missing_data_indexes(slot, 0, gap + 2, (gap + 2) as usize),
            expected,
        );
        assert_eq!(
            blocktree.find_missing_data_indexes(slot, 0, gap + 2, (gap - 1) as usize),
            &expected[..expected.len() - 1],
        );

        for i in 0..num_entries as u64 {
            for j in 0..i {
                let expected: Vec<u64> = (j..i)
                    .flat_map(|k| {
                        let begin = k * gap + 1;
                        let end = (k + 1) * gap;
                        (begin..end)
                    })
                    .collect();
                assert_eq!(
                    blocktree.find_missing_data_indexes(
                        slot,
                        j * gap,
                        i * gap,
                        ((i - j) * gap) as usize
                    ),
                    expected,
                );
            }
        }

        drop(blocktree);
        Blocktree::destroy(&blocktree_path).expect("Expected successful database destruction");
    }

    #[test]
    pub fn test_find_missing_data_indexes_slots() {
        let blocktree_path = get_tmp_ledger_path("test_find_missing_data_indexes_slots");
        let blocktree = Blocktree::open(&blocktree_path).unwrap();

        let num_entries_per_slot = 10;
        let num_slots = 2;
        let mut blobs = make_tiny_test_entries(num_slots * num_entries_per_slot).to_blobs();

        // Insert every nth entry for each slot
        let nth = 3;
        for (i, b) in blobs.iter_mut().enumerate() {
            b.set_index(((i % num_entries_per_slot) * nth) as u64);
            b.set_slot((i / num_entries_per_slot) as u64);
        }

        blocktree.write_blobs(&blobs).unwrap();

        let mut expected: Vec<u64> = (0..num_entries_per_slot)
            .flat_map(|x| ((nth * x + 1) as u64..(nth * x + nth) as u64))
            .collect();

        // For each slot, find all missing indexes in the range [0, num_entries_per_slot * nth]
        for slot_height in 0..num_slots {
            assert_eq!(
                blocktree.find_missing_data_indexes(
                    slot_height as u64,
                    0,
                    (num_entries_per_slot * nth) as u64,
                    num_entries_per_slot * nth as usize
                ),
                expected,
            );
        }

        // Test with a limit on the number of returned entries
        for slot_height in 0..num_slots {
            assert_eq!(
                blocktree.find_missing_data_indexes(
                    slot_height as u64,
                    0,
                    (num_entries_per_slot * nth) as u64,
                    num_entries_per_slot * (nth - 1)
                )[..],
                expected[..num_entries_per_slot * (nth - 1)],
            );
        }

        // Try to find entries in the range [num_entries_per_slot * nth..num_entries_per_slot * (nth + 1)
        // that don't exist in the ledger.
        let extra_entries =
            (num_entries_per_slot * nth) as u64..(num_entries_per_slot * (nth + 1)) as u64;
        expected.extend(extra_entries);

        // For each slot, find all missing indexes in the range [0, num_entries_per_slot * nth]
        for slot_height in 0..num_slots {
            assert_eq!(
                blocktree.find_missing_data_indexes(
                    slot_height as u64,
                    0,
                    (num_entries_per_slot * (nth + 1)) as u64,
                    num_entries_per_slot * (nth + 1),
                ),
                expected,
            );
        }
    }

    #[test]
    pub fn test_no_missing_blob_indexes() {
        let slot = DEFAULT_SLOT_HEIGHT;
        let blocktree_path = get_tmp_ledger_path("test_find_missing_data_indexes");
        let blocktree = Blocktree::open(&blocktree_path).unwrap();

        // Write entries
        let num_entries = 10;
        let shared_blobs = make_tiny_test_entries(num_entries).to_shared_blobs();

        index_blobs(
            &shared_blobs,
            &Keypair::new().pubkey(),
            &mut 0,
            &vec![slot; num_entries],
        );

        let blob_locks: Vec<_> = shared_blobs.iter().map(|b| b.read().unwrap()).collect();
        let blobs: Vec<&Blob> = blob_locks.iter().map(|b| &**b).collect();
        blocktree.write_blobs(blobs).unwrap();

        let empty: Vec<u64> = vec![];
        for i in 0..num_entries as u64 {
            for j in 0..i {
                assert_eq!(
                    blocktree.find_missing_data_indexes(slot, j, i, (i - j) as usize),
                    empty
                );
            }
        }

        drop(blocktree);
        Blocktree::destroy(&blocktree_path).expect("Expected successful database destruction");
    }

    #[cfg(all(feature = "erasure", test))]
    #[test]
    pub fn test_try_erasure() {
        // Setup the window
        let offset = 0;
        let num_blobs = NUM_DATA + 2;
        let slot_height = DEFAULT_SLOT_HEIGHT;
        let mut window = setup_window_ledger(offset, num_blobs, false, slot_height);
        let end_index = (offset + num_blobs) % window.len();

        // Test erasing a data block and an erasure block
        let coding_start = offset - (offset % NUM_DATA) + (NUM_DATA - NUM_CODING);

        let erased_index = coding_start % window.len();

        // Create a hole in the window
        let erased_data = window[erased_index].data.clone();
        let erased_coding = window[erased_index].coding.clone().unwrap();
        window[erased_index].data = None;
        window[erased_index].coding = None;

        // Generate the blocktree from the window
        let ledger_path = get_tmp_ledger_path("test_try_erasure");
        let blocktree = Arc::new(generate_blocktree_from_window(&ledger_path, &window, false));

        try_erasure(&blocktree, DEFAULT_SLOT_HEIGHT).expect("Expected successful erasure attempt");
        window[erased_index].data = erased_data;

        {
            let data_blobs: Vec<_> = window[erased_index..end_index]
                .iter()
                .map(|slot| slot.data.clone().unwrap())
                .collect();

            let locks: Vec<_> = data_blobs.iter().map(|blob| blob.read().unwrap()).collect();

            let locked_data: Vec<&Blob> = locks.iter().map(|lock| &**lock).collect();

            let (expected, _) = reconstruct_entries_from_blobs(locked_data).unwrap();

            assert_eq!(
                blocktree
                    .get_slot_entries(
                        0,
                        erased_index as u64,
                        Some((end_index - erased_index) as u64)
                    )
                    .unwrap(),
                expected
            );
        }

        let erased_coding_l = erased_coding.read().unwrap();
        assert_eq!(
            &blocktree
                .get_coding_blob_bytes(slot_height, erased_index as u64)
                .unwrap()
                .unwrap()[BLOB_HEADER_SIZE..],
            &erased_coding_l.data()[..erased_coding_l.size() as usize],
        );
    }

    #[test]
    fn test_process_blob() {
        let mut leader_scheduler = LeaderScheduler::default();
        leader_scheduler.set_leader_schedule(vec![Keypair::new().pubkey()]);

        let blocktree_path = get_tmp_ledger_path("test_process_blob");
        let blocktree = Arc::new(Blocktree::open(&blocktree_path).unwrap());

        let leader_scheduler = Arc::new(RwLock::new(leader_scheduler));
        let num_entries = 10;
        let original_entries = make_tiny_test_entries(num_entries);
        let shared_blobs = original_entries.clone().to_shared_blobs();

        index_blobs(
            &shared_blobs,
            &Keypair::new().pubkey(),
            &mut 0,
            &vec![DEFAULT_SLOT_HEIGHT; num_entries],
        );

        for blob in shared_blobs.iter().rev() {
            process_blob(&leader_scheduler, &blocktree, blob)
                .expect("Expect successful processing of blob");
        }

        assert_eq!(
            blocktree.get_slot_entries(0, 0, None).unwrap(),
            original_entries
        );

        drop(blocktree);
        Blocktree::destroy(&blocktree_path).expect("Expected successful database destruction");
    }
}
