//! The `repair_service` module implements the tools necessary to generate a thread which
//! regularly finds missing blobs in the ledger and sends repair requests for those blobs

use crate::bank_forks::BankForks;
use crate::blocktree::{Blocktree, CompletedSlotsReceiver, SlotMeta};
use crate::cluster_info::ClusterInfo;
use crate::result::Result;
use crate::service::Service;
use solana_metrics::datapoint;
use solana_runtime::epoch_schedule::EpochSchedule;
use solana_sdk::pubkey::Pubkey;
use std::collections::HashSet;
use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::thread::sleep;
use std::thread::{self, Builder, JoinHandle};
use std::time::Duration;

pub const MAX_REPAIR_LENGTH: usize = 16;
pub const REPAIR_MS: u64 = 100;
pub const MAX_REPAIR_TRIES: u64 = 128;
pub const NUM_FORKS_TO_REPAIR: usize = 5;
pub const MAX_ORPHANS: usize = 5;

pub enum RepairStrategy {
    RepairRange(RepairSlotRange),
    RepairAll {
        bank_forks: Arc<RwLock<BankForks>>,
        completed_slots_receiver: CompletedSlotsReceiver,
        epoch_schedule: EpochSchedule,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairType {
    Orphan(u64),
    HighestBlob(u64, u64),
    Blob(u64, u64),
}

#[derive(Default)]
struct RepairInfo {
    max_slot: u64,
    repair_tries: u64,
}

impl RepairInfo {
    fn new() -> Self {
        RepairInfo {
            max_slot: 0,
            repair_tries: 0,
        }
    }
}

pub struct RepairSlotRange {
    pub start: u64,
    pub end: u64,
}

impl Default for RepairSlotRange {
    fn default() -> Self {
        RepairSlotRange {
            start: 0,
            end: std::u64::MAX,
        }
    }
}

pub struct RepairService {
    t_repair: JoinHandle<()>,
}

impl RepairService {
    pub fn new(
        blocktree: Arc<Blocktree>,
        exit: &Arc<AtomicBool>,
        repair_socket: Arc<UdpSocket>,
        cluster_info: Arc<RwLock<ClusterInfo>>,
        repair_strategy: RepairStrategy,
    ) -> Self {
        let exit = exit.clone();
        let t_repair = Builder::new()
            .name("solana-repair-service".to_string())
            .spawn(move || {
                Self::run(
                    &blocktree,
                    exit,
                    &repair_socket,
                    &cluster_info,
                    repair_strategy,
                )
            })
            .unwrap();

        RepairService { t_repair }
    }

    fn run(
        blocktree: &Arc<Blocktree>,
        exit: Arc<AtomicBool>,
        repair_socket: &Arc<UdpSocket>,
        cluster_info: &Arc<RwLock<ClusterInfo>>,
        repair_strategy: RepairStrategy,
    ) {
        let mut repair_info = RepairInfo::new();
        let mut epoch_slots: HashSet<u64> = HashSet::new();
        let id = cluster_info.read().unwrap().id();
        if let RepairStrategy::RepairAll {
            ref bank_forks,
            ref epoch_schedule,
            ..
        } = repair_strategy
        {
            let root = bank_forks.read().unwrap().root();
            Self::initialize_epoch_slots(
                id,
                blocktree,
                &mut epoch_slots,
                root,
                epoch_schedule,
                cluster_info,
            );
        }
        loop {
            if exit.load(Ordering::Relaxed) {
                break;
            }

            let repairs = {
                match repair_strategy {
                    RepairStrategy::RepairRange(ref repair_slot_range) => {
                        // Strategy used by replicators
                        Self::generate_repairs_in_range(
                            blocktree,
                            MAX_REPAIR_LENGTH,
                            &mut repair_info,
                            repair_slot_range,
                        )
                    }

                    RepairStrategy::RepairAll {
                        ref bank_forks,
                        ref completed_slots_receiver,
                        ..
                    } => {
                        let root = bank_forks.read().unwrap().root();
                        Self::update_epoch_slots(
                            id,
                            root,
                            &mut epoch_slots,
                            &cluster_info,
                            completed_slots_receiver,
                        );
                        Self::generate_repairs(blocktree, MAX_REPAIR_LENGTH)
                    }
                }
            };

            if let Ok(repairs) = repairs {
                let reqs: Vec<_> = repairs
                    .into_iter()
                    .filter_map(|repair_request| {
                        cluster_info
                            .read()
                            .unwrap()
                            .repair_request(&repair_request)
                            .map(|result| (result, repair_request))
                            .ok()
                    })
                    .collect();

                for ((to, req), repair_request) in reqs {
                    if let Ok(local_addr) = repair_socket.local_addr() {
                        datapoint!(
                            "repair_service",
                            ("repair_request", format!("{:?}", repair_request), String),
                            ("to", to.to_string(), String),
                            ("from", local_addr.to_string(), String),
                            ("id", id.to_string(), String)
                        );
                    }
                    repair_socket.send_to(&req, to).unwrap_or_else(|e| {
                        info!("{} repair req send_to({}) error {:?}", id, to, e);
                        0
                    });
                }
            }
            sleep(Duration::from_millis(REPAIR_MS));
        }
    }

    // Generate repairs for all slots `x` in the repair_range.start <= x <= repair_range.end
    fn generate_repairs_in_range(
        blocktree: &Blocktree,
        max_repairs: usize,
        repair_info: &mut RepairInfo,
        repair_range: &RepairSlotRange,
    ) -> Result<(Vec<RepairType>)> {
        // Slot height and blob indexes for blobs we want to repair
        let mut repairs: Vec<RepairType> = vec![];
        let mut meta_iter = blocktree
            .slot_meta_iterator(repair_range.start)
            .expect("Couldn't get db iterator");
        while repairs.len() < max_repairs && meta_iter.valid() {
            let current_slot = meta_iter.key();
            if current_slot.unwrap() > repair_range.end {
                break;
            }

            if current_slot.unwrap() > repair_info.max_slot {
                repair_info.repair_tries = 0;
                repair_info.max_slot = current_slot.unwrap();
            }

            if let Some(slot) = meta_iter.value() {
                let new_repairs = Self::generate_repairs_for_slot(
                    blocktree,
                    current_slot.unwrap(),
                    &slot,
                    max_repairs - repairs.len(),
                );
                repairs.extend(new_repairs);
            }
            meta_iter.next();
        }

        // Only increment repair_tries if the ledger contains every blob for every slot
        if repairs.is_empty() {
            repair_info.repair_tries += 1;
        }

        // Optimistically try the next slot if we haven't gotten any repairs
        // for a while
        if repair_info.repair_tries >= MAX_REPAIR_TRIES {
            repairs.push(RepairType::HighestBlob(repair_info.max_slot + 1, 0))
        }

        Ok(repairs)
    }

    fn generate_repairs(blocktree: &Blocktree, max_repairs: usize) -> Result<(Vec<RepairType>)> {
        // Slot height and blob indexes for blobs we want to repair
        let mut repairs: Vec<RepairType> = vec![];
        let slot = blocktree.get_root()?;
        Self::generate_repairs_for_fork(blocktree, &mut repairs, max_repairs, slot);

        // TODO: Incorporate gossip to determine priorities for repair?

        // Try to resolve orphans in blocktree
        let orphans = blocktree.get_orphans(Some(MAX_ORPHANS));

        Self::generate_repairs_for_orphans(&orphans[..], &mut repairs);
        Ok(repairs)
    }

    fn generate_repairs_for_slot(
        blocktree: &Blocktree,
        slot: u64,
        slot_meta: &SlotMeta,
        max_repairs: usize,
    ) -> Vec<RepairType> {
        if slot_meta.is_full() {
            vec![]
        } else if slot_meta.consumed == slot_meta.received {
            vec![RepairType::HighestBlob(slot, slot_meta.received)]
        } else {
            let reqs = blocktree.find_missing_data_indexes(
                slot,
                slot_meta.consumed,
                slot_meta.received,
                max_repairs,
            );

            reqs.into_iter()
                .map(|i| RepairType::Blob(slot, i))
                .collect()
        }
    }

    fn generate_repairs_for_orphans(orphans: &[u64], repairs: &mut Vec<RepairType>) {
        repairs.extend(orphans.iter().map(|h| RepairType::Orphan(*h)));
    }

    /// Repairs any fork starting at the input slot
    fn generate_repairs_for_fork(
        blocktree: &Blocktree,
        repairs: &mut Vec<RepairType>,
        max_repairs: usize,
        slot: u64,
    ) {
        let mut pending_slots = vec![slot];
        while repairs.len() < max_repairs && !pending_slots.is_empty() {
            let slot = pending_slots.pop().unwrap();
            if let Some(slot_meta) = blocktree.meta(slot).unwrap() {
                let new_repairs = Self::generate_repairs_for_slot(
                    blocktree,
                    slot,
                    &slot_meta,
                    max_repairs - repairs.len(),
                );
                repairs.extend(new_repairs);
                let next_slots = slot_meta.next_slots;
                pending_slots.extend(next_slots);
            } else {
                break;
            }
        }
    }

    fn get_completed_slots_past_root(
        blocktree: &Blocktree,
        slots_in_gossip: &mut HashSet<u64>,
        root: u64,
        epoch_schedule: &EpochSchedule,
    ) {
        let last_confirmed_epoch = epoch_schedule.get_stakers_epoch(root);
        let last_epoch_slot = epoch_schedule.get_last_slot_in_epoch(last_confirmed_epoch);

        let mut meta_iter = blocktree
            .slot_meta_iterator(root + 1)
            .expect("Couldn't get db iterator");

        while meta_iter.valid() && meta_iter.key().unwrap() <= last_epoch_slot {
            let current_slot = meta_iter.key().unwrap();
            let meta = meta_iter.value().unwrap();
            if meta.is_full() {
                slots_in_gossip.insert(current_slot);
            }
            meta_iter.next();
        }
    }

    fn initialize_epoch_slots(
        id: Pubkey,
        blocktree: &Blocktree,
        slots_in_gossip: &mut HashSet<u64>,
        root: u64,
        epoch_schedule: &EpochSchedule,
        cluster_info: &RwLock<ClusterInfo>,
    ) {
        Self::get_completed_slots_past_root(blocktree, slots_in_gossip, root, epoch_schedule);

        // Safe to set into gossip because by this time, the leader schedule cache should
        // also be updated with the latest root (done in blocktree_processor) and thus
        // will provide a schedule to window_service for any incoming blobs up to the
        // last_confirmed_epoch.
        cluster_info
            .write()
            .unwrap()
            .push_epoch_slots(id, root, slots_in_gossip.clone());
    }

    // Update the gossiped structure used for the "Repairmen" repair protocol. See book
    // for details.
    fn update_epoch_slots(
        id: Pubkey,
        root: u64,
        slots_in_gossip: &mut HashSet<u64>,
        cluster_info: &RwLock<ClusterInfo>,
        completed_slots_receiver: &CompletedSlotsReceiver,
    ) {
        let mut should_update = false;
        while let Ok(completed_slots) = completed_slots_receiver.try_recv() {
            for slot in completed_slots {
                // If the newly completed slot > root, and the set did not contain this value
                // before, we should update gossip.
                if slot > root && slots_in_gossip.insert(slot) {
                    should_update = true;
                }
            }
        }

        if should_update {
            slots_in_gossip.retain(|x| *x > root);
            cluster_info
                .write()
                .unwrap()
                .push_epoch_slots(id, root, slots_in_gossip.clone());
        }
    }
}

impl Service for RepairService {
    type JoinReturnType = ();

    fn join(self) -> thread::Result<()> {
        self.t_repair.join()
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::blocktree::tests::{
        make_chaining_slot_entries, make_many_slot_entries, make_slot_entries,
    };
    use crate::blocktree::{get_tmp_ledger_path, Blocktree};
    use crate::cluster_info::Node;
    use rand::seq::SliceRandom;
    use rand::{thread_rng, Rng};
    use std::cmp::min;
    use std::thread::Builder;

    #[test]
    pub fn test_repair_orphan() {
        let blocktree_path = get_tmp_ledger_path!();
        {
            let blocktree = Blocktree::open(&blocktree_path).unwrap();

            // Create some orphan slots
            let (mut blobs, _) = make_slot_entries(1, 0, 1);
            let (blobs2, _) = make_slot_entries(5, 2, 1);
            blobs.extend(blobs2);
            blocktree.write_blobs(&blobs).unwrap();
            assert_eq!(
                RepairService::generate_repairs(&blocktree, 2).unwrap(),
                vec![
                    RepairType::HighestBlob(0, 0),
                    RepairType::Orphan(0),
                    RepairType::Orphan(2)
                ]
            );
        }

        Blocktree::destroy(&blocktree_path).expect("Expected successful database destruction");
    }

    #[test]
    pub fn test_repair_empty_slot() {
        let blocktree_path = get_tmp_ledger_path!();
        {
            let blocktree = Blocktree::open(&blocktree_path).unwrap();

            let (blobs, _) = make_slot_entries(2, 0, 1);

            // Write this blob to slot 2, should chain to slot 0, which we haven't received
            // any blobs for
            blocktree.write_blobs(&blobs).unwrap();

            // Check that repair tries to patch the empty slot
            assert_eq!(
                RepairService::generate_repairs(&blocktree, 2).unwrap(),
                vec![RepairType::HighestBlob(0, 0), RepairType::Orphan(0)]
            );
        }
        Blocktree::destroy(&blocktree_path).expect("Expected successful database destruction");
    }

    #[test]
    pub fn test_generate_repairs() {
        let blocktree_path = get_tmp_ledger_path!();
        {
            let blocktree = Blocktree::open(&blocktree_path).unwrap();

            let nth = 3;
            let num_entries_per_slot = 5 * nth;
            let num_slots = 2;

            // Create some blobs
            let (blobs, _) =
                make_many_slot_entries(0, num_slots as u64, num_entries_per_slot as u64);

            // write every nth blob
            let blobs_to_write: Vec<_> = blobs.iter().step_by(nth as usize).collect();

            blocktree.write_blobs(blobs_to_write).unwrap();

            let missing_indexes_per_slot: Vec<u64> = (0..num_entries_per_slot / nth - 1)
                .flat_map(|x| ((nth * x + 1) as u64..(nth * x + nth) as u64))
                .collect();

            let expected: Vec<RepairType> = (0..num_slots)
                .flat_map(|slot| {
                    missing_indexes_per_slot
                        .iter()
                        .map(move |blob_index| RepairType::Blob(slot as u64, *blob_index))
                })
                .collect();

            assert_eq!(
                RepairService::generate_repairs(&blocktree, std::usize::MAX).unwrap(),
                expected
            );

            assert_eq!(
                RepairService::generate_repairs(&blocktree, expected.len() - 2).unwrap()[..],
                expected[0..expected.len() - 2]
            );
        }
        Blocktree::destroy(&blocktree_path).expect("Expected successful database destruction");
    }

    #[test]
    pub fn test_generate_highest_repair() {
        let blocktree_path = get_tmp_ledger_path!();
        {
            let blocktree = Blocktree::open(&blocktree_path).unwrap();

            let num_entries_per_slot = 10;

            // Create some blobs
            let (mut blobs, _) = make_slot_entries(0, 0, num_entries_per_slot as u64);

            // Remove is_last flag on last blob
            blobs.last_mut().unwrap().set_flags(0);

            blocktree.write_blobs(&blobs).unwrap();

            // We didn't get the last blob for this slot, so ask for the highest blob for that slot
            let expected: Vec<RepairType> = vec![RepairType::HighestBlob(0, num_entries_per_slot)];

            assert_eq!(
                RepairService::generate_repairs(&blocktree, std::usize::MAX).unwrap(),
                expected
            );
        }
        Blocktree::destroy(&blocktree_path).expect("Expected successful database destruction");
    }

    #[test]
    pub fn test_repair_range() {
        let blocktree_path = get_tmp_ledger_path!();
        {
            let blocktree = Blocktree::open(&blocktree_path).unwrap();

            let mut repair_info = RepairInfo::new();

            let slots: Vec<u64> = vec![1, 3, 5, 7, 8];
            let num_entries_per_slot = 10;

            let blobs = make_chaining_slot_entries(&slots, num_entries_per_slot);
            for (slot_blobs, _) in blobs.iter() {
                blocktree.write_blobs(&slot_blobs[1..]).unwrap();
            }

            // Iterate through all possible combinations of start..end (inclusive on both
            // sides of the range)
            for start in 0..slots.len() {
                for end in start..slots.len() {
                    let mut repair_slot_range = RepairSlotRange::default();
                    repair_slot_range.start = slots[start];
                    repair_slot_range.end = slots[end];
                    let expected: Vec<RepairType> = slots[start..end + 1]
                        .iter()
                        .map(|slot_index| RepairType::Blob(*slot_index, 0))
                        .collect();

                    assert_eq!(
                        RepairService::generate_repairs_in_range(
                            &blocktree,
                            std::usize::MAX,
                            &mut repair_info,
                            &repair_slot_range
                        )
                        .unwrap(),
                        expected
                    );
                }
            }
        }
        Blocktree::destroy(&blocktree_path).expect("Expected successful database destruction");
    }

    #[test]
    pub fn test_repair_range_highest() {
        let blocktree_path = get_tmp_ledger_path!();
        {
            let blocktree = Blocktree::open(&blocktree_path).unwrap();

            let num_entries_per_slot = 10;

            let mut repair_info = RepairInfo::new();

            let num_slots = 1;
            let start = 5;

            // Create some blobs in slots 0..num_slots
            for i in start..start + num_slots {
                let parent = if i > 0 { i - 1 } else { 0 };
                let (blobs, _) = make_slot_entries(i, parent, num_entries_per_slot as u64);

                blocktree.write_blobs(&blobs).unwrap();
            }

            let end = 4;
            let expected: Vec<RepairType> = vec![RepairType::HighestBlob(end, 0)];

            let mut repair_slot_range = RepairSlotRange::default();
            repair_slot_range.start = 2;
            repair_slot_range.end = end;

            assert_eq!(
                RepairService::generate_repairs_in_range(
                    &blocktree,
                    std::usize::MAX,
                    &mut repair_info,
                    &repair_slot_range
                )
                .unwrap(),
                expected
            );
        }
        Blocktree::destroy(&blocktree_path).expect("Expected successful database destruction");
    }

    #[test]
    pub fn test_get_completed_slots_past_root() {
        let blocktree_path = get_tmp_ledger_path!();
        {
            let blocktree = Blocktree::open(&blocktree_path).unwrap();
            let num_entries_per_slot = 10;
            let root = 10;

            let fork1 = vec![5, 7, root, 15, 20, 21];
            let fork1_blobs: Vec<_> = make_chaining_slot_entries(&fork1, num_entries_per_slot)
                .into_iter()
                .flat_map(|(blobs, _)| blobs)
                .collect();
            let fork2 = vec![8, 12];
            let fork2_blobs = make_chaining_slot_entries(&fork2, num_entries_per_slot);

            // Remove the last blob from each slot to make an incomplete slot
            let fork2_incomplete_blobs: Vec<_> = fork2_blobs
                .into_iter()
                .flat_map(|(mut blobs, _)| {
                    blobs.pop();
                    blobs
                })
                .collect();
            let mut full_slots = HashSet::new();

            blocktree.write_blobs(&fork1_blobs).unwrap();
            blocktree.write_blobs(&fork2_incomplete_blobs).unwrap();

            // Test that only slots > root from fork1 were included
            let epoch_schedule = EpochSchedule::new(32, 32, false);

            RepairService::get_completed_slots_past_root(
                &blocktree,
                &mut full_slots,
                root,
                &epoch_schedule,
            );

            let mut expected: HashSet<_> = fork1.into_iter().filter(|x| *x > root).collect();
            assert_eq!(full_slots, expected);

            // Test that slots past the last confirmed epoch boundary don't get included
            let last_epoch = epoch_schedule.get_stakers_epoch(root);
            let last_slot = epoch_schedule.get_last_slot_in_epoch(last_epoch);
            let fork3 = vec![last_slot, last_slot + 1];
            let fork3_blobs: Vec<_> = make_chaining_slot_entries(&fork3, num_entries_per_slot)
                .into_iter()
                .flat_map(|(blobs, _)| blobs)
                .collect();
            blocktree.write_blobs(&fork3_blobs).unwrap();
            RepairService::get_completed_slots_past_root(
                &blocktree,
                &mut full_slots,
                root,
                &epoch_schedule,
            );
            expected.insert(last_slot);
            assert_eq!(full_slots, expected);
        }
        Blocktree::destroy(&blocktree_path).expect("Expected successful database destruction");
    }

    #[test]
    pub fn test_update_epoch_slots() {
        let blocktree_path = get_tmp_ledger_path!();
        {
            // Create blocktree
            let (blocktree, _, completed_slots_receiver) =
                Blocktree::open_with_signal(&blocktree_path).unwrap();

            let blocktree = Arc::new(blocktree);

            let mut root = 0;
            let num_slots = 100;
            let entries_per_slot = 5;
            let blocktree_ = blocktree.clone();

            // Spin up thread to write to blocktree
            let writer = Builder::new()
                .name("writer".to_string())
                .spawn(move || {
                    let slots: Vec<_> = (1..num_slots + 1).collect();
                    let mut blobs: Vec<_> = make_chaining_slot_entries(&slots, entries_per_slot)
                        .into_iter()
                        .flat_map(|(blobs, _)| blobs)
                        .collect();
                    blobs.shuffle(&mut thread_rng());
                    let mut i = 0;
                    let max_step = entries_per_slot * 4;
                    let repair_interval_ms = 10;
                    let mut rng = rand::thread_rng();
                    while i < blobs.len() as usize {
                        let step = rng.gen_range(1, max_step + 1);
                        blocktree_
                            .insert_data_blobs(&blobs[i..min(i + max_step as usize, blobs.len())])
                            .unwrap();
                        sleep(Duration::from_millis(repair_interval_ms));
                        i += step as usize;
                    }
                })
                .unwrap();

            let mut completed_slots = HashSet::new();
            let node_info = Node::new_localhost_with_pubkey(&Pubkey::default());
            let cluster_info = RwLock::new(ClusterInfo::new_with_invalid_keypair(
                node_info.info.clone(),
            ));

            while completed_slots.len() < num_slots as usize {
                RepairService::update_epoch_slots(
                    Pubkey::default(),
                    root,
                    &mut completed_slots,
                    &cluster_info,
                    &completed_slots_receiver,
                );
            }

            let mut expected: HashSet<_> = (1..num_slots + 1).collect();
            assert_eq!(completed_slots, expected);

            // Update with new root, should filter out the slots <= root
            root = num_slots / 2;
            let (blobs, _) = make_slot_entries(num_slots + 2, num_slots + 1, entries_per_slot);
            blocktree.insert_data_blobs(&blobs).unwrap();
            RepairService::update_epoch_slots(
                Pubkey::default(),
                root,
                &mut completed_slots,
                &cluster_info,
                &completed_slots_receiver,
            );
            expected.insert(num_slots + 2);
            expected.retain(|x| *x > root);
            assert_eq!(completed_slots, expected);
            writer.join().unwrap();
        }
        Blocktree::destroy(&blocktree_path).expect("Expected successful database destruction");
    }
}
