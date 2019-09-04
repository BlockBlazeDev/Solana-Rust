use crate::bank_forks::BankForks;
use crate::blocktree::{Blocktree, SlotMeta};
use crate::entry::{Entry, EntrySlice};
use crate::leader_schedule_cache::LeaderScheduleCache;
use rayon::prelude::*;
use rayon::ThreadPool;
use solana_metrics::{datapoint, datapoint_error, inc_new_counter_debug};
use solana_runtime::bank::Bank;
use solana_runtime::locked_accounts_results::LockedAccountsResults;
use solana_sdk::genesis_block::GenesisBlock;
use solana_sdk::hash::Hash;
use solana_sdk::timing::{duration_as_ms, Slot, MAX_RECENT_BLOCKHASHES};
use solana_sdk::transaction::Result;
use std::result;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rand::seq::SliceRandom;
use rand::thread_rng;

pub const NUM_THREADS: u32 = 10;
use std::cell::RefCell;

thread_local!(static PAR_THREAD_POOL: RefCell<ThreadPool> = RefCell::new(rayon::ThreadPoolBuilder::new()
                    .num_threads(sys_info::cpu_num().unwrap_or(NUM_THREADS) as usize)
                    .build()
                    .unwrap()));

fn first_err(results: &[Result<()>]) -> Result<()> {
    for r in results {
        if r.is_err() {
            return r.clone();
        }
    }
    Ok(())
}

fn par_execute_entries(
    bank: &Bank,
    entries: &[(&Entry, LockedAccountsResults, bool, Vec<usize>)],
) -> Result<()> {
    inc_new_counter_debug!("bank-par_execute_entries-count", entries.len());
    let results: Vec<Result<()>> = PAR_THREAD_POOL.with(|thread_pool| {
        thread_pool.borrow().install(|| {
            entries
                .into_par_iter()
                .map(
                    |(e, locked_accounts, randomize_tx_order, random_txs_execution_order)| {
                        let tx_execution_order: Option<&[usize]> = if *randomize_tx_order {
                            Some(random_txs_execution_order)
                        } else {
                            None
                        };
                        let results = bank.load_execute_and_commit_transactions(
                            &e.transactions,
                            tx_execution_order,
                            locked_accounts,
                            MAX_RECENT_BLOCKHASHES,
                        );
                        let mut first_err = None;
                        for (r, tx) in results.iter().zip(e.transactions.iter()) {
                            if let Err(ref e) = r {
                                if first_err.is_none() {
                                    first_err = Some(r.clone());
                                }
                                if !Bank::can_commit(&r) {
                                    warn!("Unexpected validator error: {:?}, tx: {:?}", e, tx);
                                    datapoint_error!(
                                        "validator_process_entry_error",
                                        ("error", format!("error: {:?}, tx: {:?}", e, tx), String)
                                    );
                                }
                            }
                        }
                        first_err.unwrap_or(Ok(()))
                    },
                )
                .collect()
        })
    });

    first_err(&results)
}

/// Process an ordered list of entries in parallel
/// 1. In order lock accounts for each entry while the lock succeeds, up to a Tick entry
/// 2. Process the locked group in parallel
/// 3. Register the `Tick` if it's available
/// 4. Update the leader scheduler, goto 1
pub fn process_entries(
    bank: &Bank,
    entries: &[Entry],
    randomize_tx_execution_order: bool,
) -> Result<()> {
    // accumulator for entries that can be processed in parallel
    let mut mt_group = vec![];
    for entry in entries {
        if entry.is_tick() {
            // if its a tick, execute the group and register the tick
            par_execute_entries(bank, &mt_group)?;
            mt_group = vec![];
            bank.register_tick(&entry.hash);
            continue;
        }
        // else loop on processing the entry
        loop {
            // random_txs_execution_order need to be seperately defined apart from txs_execution_order,
            // to satisfy borrow checker.
            let mut random_txs_execution_order: Vec<usize> = vec![];
            if randomize_tx_execution_order {
                random_txs_execution_order = (0..entry.transactions.len()).collect();
                random_txs_execution_order.shuffle(&mut thread_rng());
            }

            let txs_execution_order: Option<&[usize]> = if randomize_tx_execution_order {
                Some(&random_txs_execution_order)
            } else {
                None
            };

            // try to lock the accounts
            let lock_results = bank.lock_accounts(&entry.transactions, txs_execution_order);

            let first_lock_err = first_err(lock_results.locked_accounts_results());

            // if locking worked
            if first_lock_err.is_ok() {
                // push the entry to the mt_group
                mt_group.push((
                    entry,
                    lock_results,
                    randomize_tx_execution_order,
                    random_txs_execution_order,
                ));
                // done with this entry
                break;
            }
            // else we failed to lock, 2 possible reasons
            if mt_group.is_empty() {
                // An entry has account lock conflicts with *itself*, which should not happen
                // if generated by a properly functioning leader
                datapoint!(
                    "validator_process_entry_error",
                    (
                        "error",
                        format!(
                            "Lock accounts error, entry conflicts with itself, txs: {:?}",
                            entry.transactions
                        ),
                        String
                    )
                );
                // bail
                first_lock_err?;
            } else {
                // else we have an entry that conflicts with a prior entry
                // execute the current queue and try to process this entry again
                par_execute_entries(bank, &mt_group)?;
                mt_group = vec![];
            }
        }
    }
    par_execute_entries(bank, &mt_group)?;
    Ok(())
}

#[derive(Debug, PartialEq)]
pub struct BankForksInfo {
    pub bank_slot: u64,
}

#[derive(Debug)]
pub enum BlocktreeProcessorError {
    LedgerVerificationFailed,
}

pub fn process_blocktree(
    genesis_block: &GenesisBlock,
    blocktree: &Blocktree,
    account_paths: Option<String>,
    verify_ledger: bool,
    dev_halt_at_slot: Option<Slot>,
) -> result::Result<(BankForks, Vec<BankForksInfo>, LeaderScheduleCache), BlocktreeProcessorError> {
    info!("processing ledger from bank 0...");

    // Setup bank for slot 0
    let bank0 = Arc::new(Bank::new_with_paths(&genesis_block, account_paths));
    process_bank_0(&bank0, blocktree, verify_ledger)?;
    process_blocktree_from_root(blocktree, bank0, verify_ledger, dev_halt_at_slot)
}

// Process blocktree from a known root bank
pub fn process_blocktree_from_root(
    blocktree: &Blocktree,
    bank: Arc<Bank>,
    verify_ledger: bool,
    dev_halt_at_slot: Option<Slot>,
) -> result::Result<(BankForks, Vec<BankForksInfo>, LeaderScheduleCache), BlocktreeProcessorError> {
    info!("processing ledger from root: {}...", bank.slot());
    // Starting slot must be a root, and thus has no parents
    assert!(bank.parent().is_none());
    let start_slot = bank.slot();
    let now = Instant::now();
    let mut rooted_path = vec![start_slot];
    let dev_halt_at_slot = dev_halt_at_slot.unwrap_or(std::u64::MAX);

    blocktree
        .set_roots(&[start_slot])
        .expect("Couldn't set root on startup");

    let meta = blocktree.meta(start_slot).unwrap();

    // Iterate and replay slots from blocktree starting from `start_slot`
    let (bank_forks, bank_forks_info, leader_schedule_cache) = {
        if let Some(meta) = meta {
            let epoch_schedule = bank.epoch_schedule();
            let mut leader_schedule_cache = LeaderScheduleCache::new(*epoch_schedule, &bank);
            let fork_info = process_pending_slots(
                &bank,
                &meta,
                blocktree,
                &mut leader_schedule_cache,
                &mut rooted_path,
                verify_ledger,
                dev_halt_at_slot,
            )?;
            let (banks, bank_forks_info): (Vec<_>, Vec<_>) = fork_info.into_iter().unzip();
            let bank_forks = BankForks::new_from_banks(&banks, rooted_path);
            (bank_forks, bank_forks_info, leader_schedule_cache)
        } else {
            // If there's no meta for the input `start_slot`, then we started from a snapshot
            // and there's no point in processing the rest of blocktree and implies blocktree
            // should be empty past this point.
            let bfi = BankForksInfo {
                bank_slot: start_slot,
            };
            let leader_schedule_cache = LeaderScheduleCache::new_from_bank(&bank);
            let bank_forks = BankForks::new_from_banks(&[bank], rooted_path);
            (bank_forks, vec![bfi], leader_schedule_cache)
        }
    };

    info!(
        "processing ledger...complete in {}ms, forks={}...",
        duration_as_ms(&now.elapsed()),
        bank_forks_info.len(),
    );

    Ok((bank_forks, bank_forks_info, leader_schedule_cache))
}

fn verify_and_process_entries(
    bank: &Bank,
    entries: &[Entry],
    verify_ledger: bool,
    last_entry_hash: Hash,
) -> result::Result<Hash, BlocktreeProcessorError> {
    assert!(!entries.is_empty());

    if verify_ledger && !entries.verify(&last_entry_hash) {
        warn!("Ledger proof of history failed at slot: {}", bank.slot());
        return Err(BlocktreeProcessorError::LedgerVerificationFailed);
    }

    process_entries(&bank, &entries, true).map_err(|err| {
        warn!(
            "Failed to process entries for slot {}: {:?}",
            bank.slot(),
            err
        );
        BlocktreeProcessorError::LedgerVerificationFailed
    })?;

    Ok(entries.last().unwrap().hash)
}

// Special handling required for processing the entries in slot 0
fn process_bank_0(
    bank0: &Bank,
    blocktree: &Blocktree,
    verify_ledger: bool,
) -> result::Result<(), BlocktreeProcessorError> {
    assert_eq!(bank0.slot(), 0);

    // Fetch all entries for this slot
    let mut entries = blocktree.get_slot_entries(0, 0, None).map_err(|err| {
        warn!("Failed to load entries for slot 0, err: {:?}", err);
        BlocktreeProcessorError::LedgerVerificationFailed
    })?;

    // The first entry in the ledger is a pseudo-tick used only to ensure the number of ticks
    // in slot 0 is the same as the number of ticks in all subsequent slots.  It is not
    // processed by the bank, skip over it.
    if entries.is_empty() {
        warn!("entry0 not present");
        return Err(BlocktreeProcessorError::LedgerVerificationFailed);
    }
    let entry0 = entries.remove(0);
    if !(entry0.is_tick() && entry0.verify(&bank0.last_blockhash())) {
        warn!("Ledger proof of history failed at entry0");
        return Err(BlocktreeProcessorError::LedgerVerificationFailed);
    }

    if !entries.is_empty() {
        verify_and_process_entries(bank0, &entries, verify_ledger, entry0.hash)?;
    } else {
        bank0.register_tick(&entry0.hash);
    }

    bank0.freeze();

    Ok(())
}

// Given a slot, add its children to the pending slots queue if those children slots are
// complete
fn process_next_slots(
    bank: &Arc<Bank>,
    meta: &SlotMeta,
    blocktree: &Blocktree,
    leader_schedule_cache: &LeaderScheduleCache,
    pending_slots: &mut Vec<(u64, SlotMeta, Arc<Bank>, Hash)>,
    fork_info: &mut Vec<(Arc<Bank>, BankForksInfo)>,
) -> result::Result<(), BlocktreeProcessorError> {
    if meta.next_slots.is_empty() {
        // Reached the end of this fork.  Record the final entry height and last entry.hash
        let bfi = BankForksInfo {
            bank_slot: bank.slot(),
        };
        fork_info.push((bank.clone(), bfi));
        return Ok(());
    }

    // This is a fork point if there are multiple children, create a new child bank for each fork
    for next_slot in &meta.next_slots {
        let next_meta = blocktree
            .meta(*next_slot)
            .map_err(|err| {
                warn!("Failed to load meta for slot {}: {:?}", next_slot, err);
                BlocktreeProcessorError::LedgerVerificationFailed
            })?
            .unwrap();

        // Only process full slots in blocktree_processor, replay_stage
        // handles any partials
        if next_meta.is_full() {
            let next_bank = Arc::new(Bank::new_from_parent(
                &bank,
                &leader_schedule_cache
                    .slot_leader_at(*next_slot, Some(&bank))
                    .unwrap(),
                *next_slot,
            ));
            trace!("Add child bank {} of slot={}", next_slot, bank.slot());
            pending_slots.push((*next_slot, next_meta, next_bank, bank.last_blockhash()));
        } else {
            let bfi = BankForksInfo {
                bank_slot: bank.slot(),
            };
            fork_info.push((bank.clone(), bfi));
        }
    }

    // Reverse sort by slot, so the next slot to be processed can be popped
    // TODO: remove me once leader_scheduler can hang with out-of-order slots?
    pending_slots.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(())
}

// Iterate through blocktree processing slots starting from the root slot pointed to by the
// given `meta`
fn process_pending_slots(
    root_bank: &Arc<Bank>,
    root_meta: &SlotMeta,
    blocktree: &Blocktree,
    leader_schedule_cache: &mut LeaderScheduleCache,
    rooted_path: &mut Vec<u64>,
    verify_ledger: bool,
    dev_halt_at_slot: Slot,
) -> result::Result<Vec<(Arc<Bank>, BankForksInfo)>, BlocktreeProcessorError> {
    let mut fork_info = vec![];
    let mut last_status_report = Instant::now();
    let mut pending_slots = vec![];
    process_next_slots(
        root_bank,
        root_meta,
        blocktree,
        leader_schedule_cache,
        &mut pending_slots,
        &mut fork_info,
    )?;

    while !pending_slots.is_empty() {
        let (slot, meta, bank, last_entry_hash) = pending_slots.pop().unwrap();

        if last_status_report.elapsed() > Duration::from_secs(2) {
            info!("processing ledger...block {}", slot);
            last_status_report = Instant::now();
        }

        // Fetch all entries for this slot
        let entries = blocktree.get_slot_entries(slot, 0, None).map_err(|err| {
            warn!("Failed to load entries for slot {}: {:?}", slot, err);
            BlocktreeProcessorError::LedgerVerificationFailed
        })?;

        verify_and_process_entries(&bank, &entries, verify_ledger, last_entry_hash)?;

        bank.freeze(); // all banks handled by this routine are created from complete slots

        if blocktree.is_root(slot) {
            let parents = bank.parents().into_iter().map(|b| b.slot()).rev().skip(1);
            let parents: Vec<_> = parents.collect();
            rooted_path.extend(parents);
            rooted_path.push(slot);
            leader_schedule_cache.set_root(&bank);
            bank.squash();
            pending_slots.clear();
            fork_info.clear();
        }

        if slot >= dev_halt_at_slot {
            let bfi = BankForksInfo { bank_slot: slot };
            fork_info.push((bank, bfi));
            break;
        }

        process_next_slots(
            &bank,
            &meta,
            blocktree,
            leader_schedule_cache,
            &mut pending_slots,
            &mut fork_info,
        )?;
    }

    Ok(fork_info)
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::blocktree::create_new_tmp_ledger;
    use crate::entry::{create_ticks, next_entry, next_entry_mut, Entry};
    use crate::genesis_utils::{
        create_genesis_block, create_genesis_block_with_leader, GenesisBlockInfo,
    };
    use rand::{thread_rng, Rng};
    use solana_runtime::epoch_schedule::EpochSchedule;
    use solana_sdk::hash::Hash;
    use solana_sdk::instruction::InstructionError;
    use solana_sdk::pubkey::Pubkey;
    use solana_sdk::signature::{Keypair, KeypairUtil};
    use solana_sdk::system_transaction;
    use solana_sdk::transaction::Transaction;
    use solana_sdk::transaction::TransactionError;

    pub fn fill_blocktree_slot_with_ticks(
        blocktree: &Blocktree,
        ticks_per_slot: u64,
        slot: u64,
        parent_slot: u64,
        last_entry_hash: Hash,
    ) -> Hash {
        let entries = create_ticks(ticks_per_slot, last_entry_hash);
        let last_entry_hash = entries.last().unwrap().hash;

        blocktree
            .write_entries(
                slot,
                0,
                0,
                ticks_per_slot,
                Some(parent_slot),
                true,
                &Arc::new(Keypair::new()),
                &entries,
            )
            .unwrap();

        last_entry_hash
    }

    #[test]
    fn test_process_blocktree_with_incomplete_slot() {
        solana_logger::setup();

        let GenesisBlockInfo { genesis_block, .. } = create_genesis_block(10_000);
        let ticks_per_slot = genesis_block.ticks_per_slot;

        /*
          Build a blocktree in the ledger with the following fork structure:

               slot 0 (all ticks)
                 |
               slot 1 (all ticks but one)
                 |
               slot 2 (all ticks)

           where slot 1 is incomplete (missing 1 tick at the end)
        */

        // Create a new ledger with slot 0 full of ticks
        let (ledger_path, mut blockhash) = create_new_tmp_ledger!(&genesis_block);
        debug!("ledger_path: {:?}", ledger_path);

        let blocktree =
            Blocktree::open(&ledger_path).expect("Expected to successfully open database ledger");

        // Write slot 1
        // slot 1, points at slot 0.  Missing one tick
        {
            let parent_slot = 0;
            let slot = 1;
            let mut entries = create_ticks(ticks_per_slot, blockhash);
            blockhash = entries.last().unwrap().hash;

            // throw away last one
            entries.pop();

            blocktree
                .write_entries(
                    slot,
                    0,
                    0,
                    ticks_per_slot,
                    Some(parent_slot),
                    false,
                    &Arc::new(Keypair::new()),
                    entries,
                )
                .expect("Expected to write shredded entries to blocktree");
        }

        // slot 2, points at slot 1
        fill_blocktree_slot_with_ticks(&blocktree, ticks_per_slot, 2, 1, blockhash);

        let (mut _bank_forks, bank_forks_info, _) =
            process_blocktree(&genesis_block, &blocktree, None, true, None).unwrap();

        assert_eq!(bank_forks_info.len(), 1);
        assert_eq!(
            bank_forks_info[0],
            BankForksInfo {
                bank_slot: 0, // slot 1 isn't "full", we stop at slot zero
            }
        );
    }

    #[test]
    fn test_process_blocktree_with_two_forks_and_squash() {
        solana_logger::setup();

        let GenesisBlockInfo { genesis_block, .. } = create_genesis_block(10_000);
        let ticks_per_slot = genesis_block.ticks_per_slot;

        // Create a new ledger with slot 0 full of ticks
        let (ledger_path, blockhash) = create_new_tmp_ledger!(&genesis_block);
        debug!("ledger_path: {:?}", ledger_path);
        let mut last_entry_hash = blockhash;

        /*
            Build a blocktree in the ledger with the following fork structure:

                 slot 0
                   |
                 slot 1
                 /   \
            slot 2   |
               /     |
            slot 3   |
                     |
                   slot 4 <-- set_root(true)

        */
        let blocktree =
            Blocktree::open(&ledger_path).expect("Expected to successfully open database ledger");

        // Fork 1, ending at slot 3
        let last_slot1_entry_hash =
            fill_blocktree_slot_with_ticks(&blocktree, ticks_per_slot, 1, 0, last_entry_hash);
        last_entry_hash =
            fill_blocktree_slot_with_ticks(&blocktree, ticks_per_slot, 2, 1, last_slot1_entry_hash);
        let last_fork1_entry_hash =
            fill_blocktree_slot_with_ticks(&blocktree, ticks_per_slot, 3, 2, last_entry_hash);

        // Fork 2, ending at slot 4
        let last_fork2_entry_hash =
            fill_blocktree_slot_with_ticks(&blocktree, ticks_per_slot, 4, 1, last_slot1_entry_hash);

        info!("last_fork1_entry.hash: {:?}", last_fork1_entry_hash);
        info!("last_fork2_entry.hash: {:?}", last_fork2_entry_hash);

        blocktree.set_roots(&[0, 1, 4]).unwrap();

        let (bank_forks, bank_forks_info, _) =
            process_blocktree(&genesis_block, &blocktree, None, true, None).unwrap();

        assert_eq!(bank_forks_info.len(), 1); // One fork, other one is ignored b/c not a descendant of the root

        assert_eq!(
            bank_forks_info[0],
            BankForksInfo {
                bank_slot: 4, // Fork 2's head is slot 4
            }
        );
        assert!(&bank_forks[4]
            .parents()
            .iter()
            .map(|bank| bank.slot())
            .collect::<Vec<_>>()
            .is_empty());

        // Ensure bank_forks holds the right banks
        verify_fork_infos(&bank_forks, &bank_forks_info);

        assert_eq!(bank_forks.root(), 4);
    }

    #[test]
    fn test_process_blocktree_with_two_forks() {
        solana_logger::setup();

        let GenesisBlockInfo { genesis_block, .. } = create_genesis_block(10_000);
        let ticks_per_slot = genesis_block.ticks_per_slot;

        // Create a new ledger with slot 0 full of ticks
        let (ledger_path, blockhash) = create_new_tmp_ledger!(&genesis_block);
        debug!("ledger_path: {:?}", ledger_path);
        let mut last_entry_hash = blockhash;

        /*
            Build a blocktree in the ledger with the following fork structure:

                 slot 0
                   |
                 slot 1  <-- set_root(true)
                 /   \
            slot 2   |
               /     |
            slot 3   |
                     |
                   slot 4

        */
        let blocktree =
            Blocktree::open(&ledger_path).expect("Expected to successfully open database ledger");

        // Fork 1, ending at slot 3
        let last_slot1_entry_hash =
            fill_blocktree_slot_with_ticks(&blocktree, ticks_per_slot, 1, 0, last_entry_hash);
        last_entry_hash =
            fill_blocktree_slot_with_ticks(&blocktree, ticks_per_slot, 2, 1, last_slot1_entry_hash);
        let last_fork1_entry_hash =
            fill_blocktree_slot_with_ticks(&blocktree, ticks_per_slot, 3, 2, last_entry_hash);

        // Fork 2, ending at slot 4
        let last_fork2_entry_hash =
            fill_blocktree_slot_with_ticks(&blocktree, ticks_per_slot, 4, 1, last_slot1_entry_hash);

        info!("last_fork1_entry.hash: {:?}", last_fork1_entry_hash);
        info!("last_fork2_entry.hash: {:?}", last_fork2_entry_hash);

        blocktree.set_roots(&[0, 1]).unwrap();

        let (bank_forks, bank_forks_info, _) =
            process_blocktree(&genesis_block, &blocktree, None, true, None).unwrap();

        assert_eq!(bank_forks_info.len(), 2); // There are two forks
        assert_eq!(
            bank_forks_info[0],
            BankForksInfo {
                bank_slot: 3, // Fork 1's head is slot 3
            }
        );
        assert_eq!(
            &bank_forks[3]
                .parents()
                .iter()
                .map(|bank| bank.slot())
                .collect::<Vec<_>>(),
            &[2, 1]
        );
        assert_eq!(
            bank_forks_info[1],
            BankForksInfo {
                bank_slot: 4, // Fork 2's head is slot 4
            }
        );
        assert_eq!(
            &bank_forks[4]
                .parents()
                .iter()
                .map(|bank| bank.slot())
                .collect::<Vec<_>>(),
            &[1]
        );

        assert_eq!(bank_forks.root(), 1);

        // Ensure bank_forks holds the right banks
        verify_fork_infos(&bank_forks, &bank_forks_info);
    }

    #[test]
    fn test_process_blocktree_epoch_boundary_root() {
        solana_logger::setup();

        let GenesisBlockInfo { genesis_block, .. } = create_genesis_block(10_000);
        let ticks_per_slot = genesis_block.ticks_per_slot;

        // Create a new ledger with slot 0 full of ticks
        let (ledger_path, blockhash) = create_new_tmp_ledger!(&genesis_block);
        let mut last_entry_hash = blockhash;

        let blocktree =
            Blocktree::open(&ledger_path).expect("Expected to successfully open database ledger");

        // Let last_slot be the number of slots in the first two epochs
        let epoch_schedule = get_epoch_schedule(&genesis_block, None);
        let last_slot = epoch_schedule.get_last_slot_in_epoch(1);

        // Create a single chain of slots with all indexes in the range [0, last_slot + 1]
        for i in 1..=last_slot + 1 {
            last_entry_hash = fill_blocktree_slot_with_ticks(
                &blocktree,
                ticks_per_slot,
                i,
                i - 1,
                last_entry_hash,
            );
        }

        // Set a root on the last slot of the last confirmed epoch
        let rooted_slots: Vec<_> = (0..=last_slot).collect();
        blocktree.set_roots(&rooted_slots).unwrap();

        // Set a root on the next slot of the confrimed epoch
        blocktree.set_roots(&[last_slot + 1]).unwrap();

        // Check that we can properly restart the ledger / leader scheduler doesn't fail
        let (bank_forks, bank_forks_info, _) =
            process_blocktree(&genesis_block, &blocktree, None, true, None).unwrap();

        assert_eq!(bank_forks_info.len(), 1); // There is one fork
        assert_eq!(
            bank_forks_info[0],
            BankForksInfo {
                bank_slot: last_slot + 1, // Head is last_slot + 1
            }
        );

        // The latest root should have purged all its parents
        assert!(&bank_forks[last_slot + 1]
            .parents()
            .iter()
            .map(|bank| bank.slot())
            .collect::<Vec<_>>()
            .is_empty());
    }

    #[test]
    fn test_first_err() {
        assert_eq!(first_err(&[Ok(())]), Ok(()));
        assert_eq!(
            first_err(&[Ok(()), Err(TransactionError::DuplicateSignature)]),
            Err(TransactionError::DuplicateSignature)
        );
        assert_eq!(
            first_err(&[
                Ok(()),
                Err(TransactionError::DuplicateSignature),
                Err(TransactionError::AccountInUse)
            ]),
            Err(TransactionError::DuplicateSignature)
        );
        assert_eq!(
            first_err(&[
                Ok(()),
                Err(TransactionError::AccountInUse),
                Err(TransactionError::DuplicateSignature)
            ]),
            Err(TransactionError::AccountInUse)
        );
        assert_eq!(
            first_err(&[
                Err(TransactionError::AccountInUse),
                Ok(()),
                Err(TransactionError::DuplicateSignature)
            ]),
            Err(TransactionError::AccountInUse)
        );
    }

    #[test]
    fn test_process_empty_entry_is_registered() {
        solana_logger::setup();

        let GenesisBlockInfo {
            genesis_block,
            mint_keypair,
            ..
        } = create_genesis_block(2);
        let bank = Bank::new(&genesis_block);
        let keypair = Keypair::new();
        let slot_entries = create_ticks(genesis_block.ticks_per_slot - 1, genesis_block.hash());
        let tx = system_transaction::create_user_account(
            &mint_keypair,
            &keypair.pubkey(),
            1,
            slot_entries.last().unwrap().hash,
        );

        // First, ensure the TX is rejected because of the unregistered last ID
        assert_eq!(
            bank.process_transaction(&tx),
            Err(TransactionError::BlockhashNotFound)
        );

        // Now ensure the TX is accepted despite pointing to the ID of an empty entry.
        process_entries(&bank, &slot_entries, true).unwrap();
        assert_eq!(bank.process_transaction(&tx), Ok(()));
    }

    #[test]
    fn test_process_ledger_simple() {
        solana_logger::setup();
        let leader_pubkey = Pubkey::new_rand();
        let mint = 100;
        let GenesisBlockInfo {
            genesis_block,
            mint_keypair,
            ..
        } = create_genesis_block_with_leader(mint, &leader_pubkey, 50);
        let (ledger_path, mut last_entry_hash) = create_new_tmp_ledger!(&genesis_block);
        debug!("ledger_path: {:?}", ledger_path);

        let deducted_from_mint = 3;
        let mut entries = vec![];
        let blockhash = genesis_block.hash();
        for _ in 0..deducted_from_mint {
            // Transfer one token from the mint to a random account
            let keypair = Keypair::new();
            let tx = system_transaction::create_user_account(
                &mint_keypair,
                &keypair.pubkey(),
                1,
                blockhash,
            );
            let entry = Entry::new(&last_entry_hash, 1, vec![tx]);
            last_entry_hash = entry.hash;
            entries.push(entry);

            // Add a second Transaction that will produce a
            // InstructionError<0, ResultWithNegativeLamports> error when processed
            let keypair2 = Keypair::new();
            let tx = system_transaction::create_user_account(
                &keypair,
                &keypair2.pubkey(),
                42,
                blockhash,
            );
            let entry = Entry::new(&last_entry_hash, 1, vec![tx]);
            last_entry_hash = entry.hash;
            entries.push(entry);
        }

        // Fill up the rest of slot 1 with ticks
        entries.extend(create_ticks(genesis_block.ticks_per_slot, last_entry_hash));

        let blocktree =
            Blocktree::open(&ledger_path).expect("Expected to successfully open database ledger");
        blocktree
            .write_entries(
                1,
                0,
                0,
                genesis_block.ticks_per_slot,
                None,
                true,
                &Arc::new(Keypair::new()),
                &entries,
            )
            .unwrap();
        let (bank_forks, bank_forks_info, _) =
            process_blocktree(&genesis_block, &blocktree, None, true, None).unwrap();

        assert_eq!(bank_forks_info.len(), 1);
        assert_eq!(bank_forks.root(), 0);
        assert_eq!(bank_forks_info[0], BankForksInfo { bank_slot: 1 });

        let bank = bank_forks[1].clone();
        assert_eq!(
            bank.get_balance(&mint_keypair.pubkey()),
            mint - deducted_from_mint
        );
        assert_eq!(bank.tick_height(), 2 * genesis_block.ticks_per_slot - 1);
        assert_eq!(bank.last_blockhash(), entries.last().unwrap().hash);
    }

    #[test]
    fn test_process_ledger_with_one_tick_per_slot() {
        let GenesisBlockInfo {
            mut genesis_block, ..
        } = create_genesis_block(123);
        genesis_block.ticks_per_slot = 1;
        let (ledger_path, _blockhash) = create_new_tmp_ledger!(&genesis_block);

        let blocktree = Blocktree::open(&ledger_path).unwrap();
        let (bank_forks, bank_forks_info, _) =
            process_blocktree(&genesis_block, &blocktree, None, true, None).unwrap();

        assert_eq!(bank_forks_info.len(), 1);
        assert_eq!(bank_forks_info[0], BankForksInfo { bank_slot: 0 });
        let bank = bank_forks[0].clone();
        assert_eq!(bank.tick_height(), 0);
    }

    #[test]
    fn test_process_entries_tick() {
        let GenesisBlockInfo { genesis_block, .. } = create_genesis_block(1000);
        let bank = Bank::new(&genesis_block);

        // ensure bank can process a tick
        assert_eq!(bank.tick_height(), 0);
        let tick = next_entry(&genesis_block.hash(), 1, vec![]);
        assert_eq!(process_entries(&bank, &[tick.clone()], true), Ok(()));
        assert_eq!(bank.tick_height(), 1);
    }

    #[test]
    fn test_process_entries_2_entries_collision() {
        let GenesisBlockInfo {
            genesis_block,
            mint_keypair,
            ..
        } = create_genesis_block(1000);
        let bank = Bank::new(&genesis_block);
        let keypair1 = Keypair::new();
        let keypair2 = Keypair::new();

        let blockhash = bank.last_blockhash();

        // ensure bank can process 2 entries that have a common account and no tick is registered
        let tx = system_transaction::create_user_account(
            &mint_keypair,
            &keypair1.pubkey(),
            2,
            bank.last_blockhash(),
        );
        let entry_1 = next_entry(&blockhash, 1, vec![tx]);
        let tx = system_transaction::create_user_account(
            &mint_keypair,
            &keypair2.pubkey(),
            2,
            bank.last_blockhash(),
        );
        let entry_2 = next_entry(&entry_1.hash, 1, vec![tx]);
        assert_eq!(process_entries(&bank, &[entry_1, entry_2], true), Ok(()));
        assert_eq!(bank.get_balance(&keypair1.pubkey()), 2);
        assert_eq!(bank.get_balance(&keypair2.pubkey()), 2);
        assert_eq!(bank.last_blockhash(), blockhash);
    }

    #[test]
    fn test_process_entries_2_txes_collision() {
        let GenesisBlockInfo {
            genesis_block,
            mint_keypair,
            ..
        } = create_genesis_block(1000);
        let bank = Bank::new(&genesis_block);
        let keypair1 = Keypair::new();
        let keypair2 = Keypair::new();
        let keypair3 = Keypair::new();

        // fund: put 4 in each of 1 and 2
        assert_matches!(bank.transfer(4, &mint_keypair, &keypair1.pubkey()), Ok(_));
        assert_matches!(bank.transfer(4, &mint_keypair, &keypair2.pubkey()), Ok(_));

        // construct an Entry whose 2nd transaction would cause a lock conflict with previous entry
        let entry_1_to_mint = next_entry(
            &bank.last_blockhash(),
            1,
            vec![system_transaction::create_user_account(
                &keypair1,
                &mint_keypair.pubkey(),
                1,
                bank.last_blockhash(),
            )],
        );

        let entry_2_to_3_mint_to_1 = next_entry(
            &entry_1_to_mint.hash,
            1,
            vec![
                system_transaction::create_user_account(
                    &keypair2,
                    &keypair3.pubkey(),
                    2,
                    bank.last_blockhash(),
                ), // should be fine
                system_transaction::create_user_account(
                    &keypair1,
                    &mint_keypair.pubkey(),
                    2,
                    bank.last_blockhash(),
                ), // will collide
            ],
        );

        assert_eq!(
            process_entries(&bank, &[entry_1_to_mint, entry_2_to_3_mint_to_1], false),
            Ok(())
        );

        assert_eq!(bank.get_balance(&keypair1.pubkey()), 1);
        assert_eq!(bank.get_balance(&keypair2.pubkey()), 2);
        assert_eq!(bank.get_balance(&keypair3.pubkey()), 2);
    }

    #[test]
    fn test_process_entries_2_txes_collision_and_error() {
        let GenesisBlockInfo {
            genesis_block,
            mint_keypair,
            ..
        } = create_genesis_block(1000);
        let bank = Bank::new(&genesis_block);
        let keypair1 = Keypair::new();
        let keypair2 = Keypair::new();
        let keypair3 = Keypair::new();
        let keypair4 = Keypair::new();

        // fund: put 4 in each of 1 and 2
        assert_matches!(bank.transfer(4, &mint_keypair, &keypair1.pubkey()), Ok(_));
        assert_matches!(bank.transfer(4, &mint_keypair, &keypair2.pubkey()), Ok(_));
        assert_matches!(bank.transfer(4, &mint_keypair, &keypair4.pubkey()), Ok(_));

        // construct an Entry whose 2nd transaction would cause a lock conflict with previous entry
        let entry_1_to_mint = next_entry(
            &bank.last_blockhash(),
            1,
            vec![
                system_transaction::create_user_account(
                    &keypair1,
                    &mint_keypair.pubkey(),
                    1,
                    bank.last_blockhash(),
                ),
                system_transaction::transfer(
                    &keypair4,
                    &keypair4.pubkey(),
                    1,
                    Hash::default(), // Should cause a transaction failure with BlockhashNotFound
                ),
            ],
        );

        let entry_2_to_3_mint_to_1 = next_entry(
            &entry_1_to_mint.hash,
            1,
            vec![
                system_transaction::create_user_account(
                    &keypair2,
                    &keypair3.pubkey(),
                    2,
                    bank.last_blockhash(),
                ), // should be fine
                system_transaction::create_user_account(
                    &keypair1,
                    &mint_keypair.pubkey(),
                    2,
                    bank.last_blockhash(),
                ), // will collide
            ],
        );

        assert!(process_entries(
            &bank,
            &[entry_1_to_mint.clone(), entry_2_to_3_mint_to_1.clone()],
            false
        )
        .is_err());

        // First transaction in first entry succeeded, so keypair1 lost 1 lamport
        assert_eq!(bank.get_balance(&keypair1.pubkey()), 3);
        assert_eq!(bank.get_balance(&keypair2.pubkey()), 4);

        // Check all accounts are unlocked
        let txs1 = &entry_1_to_mint.transactions[..];
        let txs2 = &entry_2_to_3_mint_to_1.transactions[..];
        let locked_accounts1 = bank.lock_accounts(txs1, None);
        for result in locked_accounts1.locked_accounts_results() {
            assert!(result.is_ok());
        }
        // txs1 and txs2 have accounts that conflict, so we must drop txs1 first
        drop(locked_accounts1);
        let locked_accounts2 = bank.lock_accounts(txs2, None);
        for result in locked_accounts2.locked_accounts_results() {
            assert!(result.is_ok());
        }
    }

    #[test]
    fn test_process_entries_2nd_entry_collision_with_self_and_error() {
        solana_logger::setup();

        let GenesisBlockInfo {
            genesis_block,
            mint_keypair,
            ..
        } = create_genesis_block(1000);
        let bank = Bank::new(&genesis_block);
        let keypair1 = Keypair::new();
        let keypair2 = Keypair::new();
        let keypair3 = Keypair::new();

        // fund: put some money in each of 1 and 2
        assert_matches!(bank.transfer(5, &mint_keypair, &keypair1.pubkey()), Ok(_));
        assert_matches!(bank.transfer(4, &mint_keypair, &keypair2.pubkey()), Ok(_));

        // 3 entries: first has a transfer, 2nd has a conflict with 1st, 3rd has a conflict with itself
        let entry_1_to_mint = next_entry(
            &bank.last_blockhash(),
            1,
            vec![system_transaction::transfer(
                &keypair1,
                &mint_keypair.pubkey(),
                1,
                bank.last_blockhash(),
            )],
        );
        // should now be:
        // keypair1=4
        // keypair2=4
        // keypair3=0

        let entry_2_to_3_and_1_to_mint = next_entry(
            &entry_1_to_mint.hash,
            1,
            vec![
                system_transaction::create_user_account(
                    &keypair2,
                    &keypair3.pubkey(),
                    2,
                    bank.last_blockhash(),
                ), // should be fine
                system_transaction::transfer(
                    &keypair1,
                    &mint_keypair.pubkey(),
                    2,
                    bank.last_blockhash(),
                ), // will collide with predecessor
            ],
        );
        // should now be:
        // keypair1=2
        // keypair2=2
        // keypair3=2

        let entry_conflict_itself = next_entry(
            &entry_2_to_3_and_1_to_mint.hash,
            1,
            vec![
                system_transaction::transfer(
                    &keypair1,
                    &keypair3.pubkey(),
                    1,
                    bank.last_blockhash(),
                ),
                system_transaction::transfer(
                    &keypair1,
                    &keypair2.pubkey(),
                    1,
                    bank.last_blockhash(),
                ), // should be fine
            ],
        );
        // would now be:
        // keypair1=0
        // keypair2=3
        // keypair3=3

        assert!(process_entries(
            &bank,
            &[
                entry_1_to_mint.clone(),
                entry_2_to_3_and_1_to_mint.clone(),
                entry_conflict_itself.clone()
            ],
            false
        )
        .is_err());

        // last entry should have been aborted before par_execute_entries
        assert_eq!(bank.get_balance(&keypair1.pubkey()), 2);
        assert_eq!(bank.get_balance(&keypair2.pubkey()), 2);
        assert_eq!(bank.get_balance(&keypair3.pubkey()), 2);
    }

    #[test]
    fn test_process_entries_2_entries_par() {
        let GenesisBlockInfo {
            genesis_block,
            mint_keypair,
            ..
        } = create_genesis_block(1000);
        let bank = Bank::new(&genesis_block);
        let keypair1 = Keypair::new();
        let keypair2 = Keypair::new();
        let keypair3 = Keypair::new();
        let keypair4 = Keypair::new();

        //load accounts
        let tx = system_transaction::create_user_account(
            &mint_keypair,
            &keypair1.pubkey(),
            1,
            bank.last_blockhash(),
        );
        assert_eq!(bank.process_transaction(&tx), Ok(()));
        let tx = system_transaction::create_user_account(
            &mint_keypair,
            &keypair2.pubkey(),
            1,
            bank.last_blockhash(),
        );
        assert_eq!(bank.process_transaction(&tx), Ok(()));

        // ensure bank can process 2 entries that do not have a common account and no tick is registered
        let blockhash = bank.last_blockhash();
        let tx = system_transaction::create_user_account(
            &keypair1,
            &keypair3.pubkey(),
            1,
            bank.last_blockhash(),
        );
        let entry_1 = next_entry(&blockhash, 1, vec![tx]);
        let tx = system_transaction::create_user_account(
            &keypair2,
            &keypair4.pubkey(),
            1,
            bank.last_blockhash(),
        );
        let entry_2 = next_entry(&entry_1.hash, 1, vec![tx]);
        assert_eq!(process_entries(&bank, &[entry_1, entry_2], true), Ok(()));
        assert_eq!(bank.get_balance(&keypair3.pubkey()), 1);
        assert_eq!(bank.get_balance(&keypair4.pubkey()), 1);
        assert_eq!(bank.last_blockhash(), blockhash);
    }

    #[test]
    fn test_process_entry_tx_random_execution_no_error() {
        // entropy multiplier should be big enough to provide sufficient entropy
        // but small enough to not take too much time while executing the test.
        let entropy_multiplier: usize = 25;
        let initial_lamports = 100;

        // number of accounts need to be in multiple of 4 for correct
        // execution of the test.
        let num_accounts = entropy_multiplier * 4;
        let GenesisBlockInfo {
            genesis_block,
            mint_keypair,
            ..
        } = create_genesis_block((num_accounts + 1) as u64 * initial_lamports);

        let bank = Bank::new(&genesis_block);

        let mut keypairs: Vec<Keypair> = vec![];

        for _ in 0..num_accounts {
            let keypair = Keypair::new();
            let create_account_tx = system_transaction::create_user_account(
                &mint_keypair,
                &keypair.pubkey(),
                0,
                bank.last_blockhash(),
            );
            assert_eq!(bank.process_transaction(&create_account_tx), Ok(()));
            assert_matches!(
                bank.transfer(initial_lamports, &mint_keypair, &keypair.pubkey()),
                Ok(_)
            );
            keypairs.push(keypair);
        }

        let mut tx_vector: Vec<Transaction> = vec![];

        for i in (0..num_accounts).step_by(4) {
            tx_vector.append(&mut vec![
                system_transaction::transfer(
                    &keypairs[i + 1],
                    &keypairs[i].pubkey(),
                    initial_lamports,
                    bank.last_blockhash(),
                ),
                system_transaction::transfer(
                    &keypairs[i + 3],
                    &keypairs[i + 2].pubkey(),
                    initial_lamports,
                    bank.last_blockhash(),
                ),
            ]);
        }

        // Transfer lamports to each other
        let entry = next_entry(&bank.last_blockhash(), 1, tx_vector);
        assert_eq!(process_entries(&bank, &vec![entry], true), Ok(()));
        bank.squash();

        // Even number keypair should have balance of 2 * initial_lamports and
        // odd number keypair should have balance of 0, which proves
        // that even in case of random order of execution, overall state remains
        // consistent.
        for i in 0..num_accounts {
            if i % 2 == 0 {
                assert_eq!(
                    bank.get_balance(&keypairs[i].pubkey()),
                    2 * initial_lamports
                );
            } else {
                assert_eq!(bank.get_balance(&keypairs[i].pubkey()), 0);
            }
        }
    }

    #[test]
    fn test_process_entries_2_entries_tick() {
        let GenesisBlockInfo {
            genesis_block,
            mint_keypair,
            ..
        } = create_genesis_block(1000);
        let bank = Bank::new(&genesis_block);
        let keypair1 = Keypair::new();
        let keypair2 = Keypair::new();
        let keypair3 = Keypair::new();
        let keypair4 = Keypair::new();

        //load accounts
        let tx = system_transaction::create_user_account(
            &mint_keypair,
            &keypair1.pubkey(),
            1,
            bank.last_blockhash(),
        );
        assert_eq!(bank.process_transaction(&tx), Ok(()));
        let tx = system_transaction::create_user_account(
            &mint_keypair,
            &keypair2.pubkey(),
            1,
            bank.last_blockhash(),
        );
        assert_eq!(bank.process_transaction(&tx), Ok(()));

        let blockhash = bank.last_blockhash();
        while blockhash == bank.last_blockhash() {
            bank.register_tick(&Hash::default());
        }

        // ensure bank can process 2 entries that do not have a common account and tick is registered
        let tx =
            system_transaction::create_user_account(&keypair2, &keypair3.pubkey(), 1, blockhash);
        let entry_1 = next_entry(&blockhash, 1, vec![tx]);
        let tick = next_entry(&entry_1.hash, 1, vec![]);
        let tx = system_transaction::create_user_account(
            &keypair1,
            &keypair4.pubkey(),
            1,
            bank.last_blockhash(),
        );
        let entry_2 = next_entry(&tick.hash, 1, vec![tx]);
        assert_eq!(
            process_entries(
                &bank,
                &[entry_1.clone(), tick.clone(), entry_2.clone()],
                true
            ),
            Ok(())
        );
        assert_eq!(bank.get_balance(&keypair3.pubkey()), 1);
        assert_eq!(bank.get_balance(&keypair4.pubkey()), 1);

        // ensure that an error is returned for an empty account (keypair2)
        let tx = system_transaction::create_user_account(
            &keypair2,
            &keypair3.pubkey(),
            1,
            bank.last_blockhash(),
        );
        let entry_3 = next_entry(&entry_2.hash, 1, vec![tx]);
        assert_eq!(
            process_entries(&bank, &[entry_3], true),
            Err(TransactionError::AccountNotFound)
        );
    }

    #[test]
    fn test_update_transaction_statuses() {
        // Make sure instruction errors still update the signature cache
        let GenesisBlockInfo {
            genesis_block,
            mint_keypair,
            ..
        } = create_genesis_block(11_000);
        let bank = Bank::new(&genesis_block);
        let pubkey = Pubkey::new_rand();
        bank.transfer(1_000, &mint_keypair, &pubkey).unwrap();
        assert_eq!(bank.transaction_count(), 1);
        assert_eq!(bank.get_balance(&pubkey), 1_000);
        assert_eq!(
            bank.transfer(10_001, &mint_keypair, &pubkey),
            Err(TransactionError::InstructionError(
                0,
                InstructionError::new_result_with_negative_lamports(),
            ))
        );
        assert_eq!(
            bank.transfer(10_001, &mint_keypair, &pubkey),
            Err(TransactionError::DuplicateSignature)
        );

        // Make sure other errors don't update the signature cache
        let tx =
            system_transaction::create_user_account(&mint_keypair, &pubkey, 1000, Hash::default());
        let signature = tx.signatures[0];

        // Should fail with blockhash not found
        assert_eq!(
            bank.process_transaction(&tx).map(|_| signature),
            Err(TransactionError::BlockhashNotFound)
        );

        // Should fail again with blockhash not found
        assert_eq!(
            bank.process_transaction(&tx).map(|_| signature),
            Err(TransactionError::BlockhashNotFound)
        );
    }

    #[test]
    fn test_update_transaction_statuses_fail() {
        let GenesisBlockInfo {
            genesis_block,
            mint_keypair,
            ..
        } = create_genesis_block(11_000);
        let bank = Bank::new(&genesis_block);
        let keypair1 = Keypair::new();
        let keypair2 = Keypair::new();
        let success_tx = system_transaction::create_user_account(
            &mint_keypair,
            &keypair1.pubkey(),
            1,
            bank.last_blockhash(),
        );
        let fail_tx = system_transaction::create_user_account(
            &mint_keypair,
            &keypair2.pubkey(),
            2,
            bank.last_blockhash(),
        );

        let entry_1_to_mint = next_entry(
            &bank.last_blockhash(),
            1,
            vec![
                success_tx,
                fail_tx.clone(), // will collide
            ],
        );

        assert_eq!(
            process_entries(&bank, &[entry_1_to_mint], false),
            Err(TransactionError::AccountInUse)
        );

        // Should not see duplicate signature error
        assert_eq!(bank.process_transaction(&fail_tx), Ok(()));
    }

    #[test]
    fn test_process_blocktree_from_root() {
        let GenesisBlockInfo {
            mut genesis_block, ..
        } = create_genesis_block(123);

        let ticks_per_slot = 1;
        genesis_block.ticks_per_slot = ticks_per_slot;
        let (ledger_path, blockhash) = create_new_tmp_ledger!(&genesis_block);
        let blocktree = Blocktree::open(&ledger_path).unwrap();

        /*
          Build a blocktree in the ledger with the following fork structure:

               slot 0 (all ticks)
                 |
               slot 1 (all ticks)
                 |
               slot 2 (all ticks)
                 |
               slot 3 (all ticks) -> root
                 |
               slot 4 (all ticks)
                 |
               slot 5 (all ticks) -> root
                 |
               slot 6 (all ticks)
        */

        let mut last_hash = blockhash;
        for i in 0..6 {
            last_hash =
                fill_blocktree_slot_with_ticks(&blocktree, ticks_per_slot, i + 1, i, last_hash);
        }
        blocktree.set_roots(&[3, 5]).unwrap();

        // Set up bank1
        let bank0 = Arc::new(Bank::new(&genesis_block));
        process_bank_0(&bank0, &blocktree, true).unwrap();
        let bank1 = Arc::new(Bank::new_from_parent(&bank0, &Pubkey::default(), 1));
        bank1.squash();
        let slot1_entries = blocktree.get_slot_entries(1, 0, None).unwrap();
        verify_and_process_entries(&bank1, &slot1_entries, true, bank0.last_blockhash()).unwrap();

        // Test process_blocktree_from_root() from slot 1 onwards
        let (bank_forks, bank_forks_info, _) =
            process_blocktree_from_root(&blocktree, bank1, true, None).unwrap();

        assert_eq!(bank_forks_info.len(), 1); // One fork
        assert_eq!(
            bank_forks_info[0],
            BankForksInfo {
                bank_slot: 6, // The head of the fork is slot 6
            }
        );

        // slots_since_snapshot should contain everything on the rooted path
        assert_eq!(
            bank_forks.slots_since_snapshot().to_vec(),
            vec![1, 2, 3, 4, 5]
        );
        assert_eq!(bank_forks.root(), 5);

        // Verify the parents of the head of the fork
        assert_eq!(
            &bank_forks[6]
                .parents()
                .iter()
                .map(|bank| bank.slot())
                .collect::<Vec<_>>(),
            &[5]
        );

        // Check that bank forks has the correct banks
        verify_fork_infos(&bank_forks, &bank_forks_info);
    }

    #[test]
    #[ignore]
    fn test_process_entries_stress() {
        // this test throws lots of rayon threads at process_entries()
        //  finds bugs in very low-layer stuff
        solana_logger::setup();
        let GenesisBlockInfo {
            genesis_block,
            mint_keypair,
            ..
        } = create_genesis_block(1_000_000_000);
        let mut bank = Bank::new(&genesis_block);

        const NUM_TRANSFERS: usize = 128;
        let keypairs: Vec<_> = (0..NUM_TRANSFERS * 2).map(|_| Keypair::new()).collect();

        // give everybody one lamport
        for keypair in &keypairs {
            bank.transfer(1, &mint_keypair, &keypair.pubkey())
                .expect("funding failed");
        }

        let mut i = 0;
        let mut hash = bank.last_blockhash();
        let mut root: Option<Arc<Bank>> = None;
        loop {
            let entries: Vec<_> = (0..NUM_TRANSFERS)
                .map(|i| {
                    next_entry_mut(
                        &mut hash,
                        0,
                        vec![system_transaction::transfer(
                            &keypairs[i],
                            &keypairs[i + NUM_TRANSFERS].pubkey(),
                            1,
                            bank.last_blockhash(),
                        )],
                    )
                })
                .collect();
            info!("paying iteration {}", i);
            process_entries(&bank, &entries, true).expect("paying failed");

            let entries: Vec<_> = (0..NUM_TRANSFERS)
                .map(|i| {
                    next_entry_mut(
                        &mut hash,
                        0,
                        vec![system_transaction::transfer(
                            &keypairs[i + NUM_TRANSFERS],
                            &keypairs[i].pubkey(),
                            1,
                            bank.last_blockhash(),
                        )],
                    )
                })
                .collect();

            info!("refunding iteration {}", i);
            process_entries(&bank, &entries, true).expect("refunding failed");

            // advance to next block
            process_entries(
                &bank,
                &(0..bank.ticks_per_slot())
                    .map(|_| next_entry_mut(&mut hash, 1, vec![]))
                    .collect::<Vec<_>>(),
                true,
            )
            .expect("process ticks failed");

            let parent = Arc::new(bank);

            if i % 16 == 0 {
                root.map(|old_root| old_root.squash());
                root = Some(parent.clone());
            }
            i += 1;

            bank = Bank::new_from_parent(
                &parent,
                &Pubkey::default(),
                parent.slot() + thread_rng().gen_range(1, 3),
            );
        }
    }

    fn get_epoch_schedule(
        genesis_block: &GenesisBlock,
        account_paths: Option<String>,
    ) -> EpochSchedule {
        let bank = Bank::new_with_paths(&genesis_block, account_paths);
        bank.epoch_schedule().clone()
    }

    // Check that `bank_forks` contains all the ancestors and banks for each fork identified in
    // `bank_forks_info`
    fn verify_fork_infos(bank_forks: &BankForks, bank_forks_info: &[BankForksInfo]) {
        for fork in bank_forks_info {
            let head_slot = fork.bank_slot;
            let head_bank = &bank_forks[head_slot];
            let mut parents = head_bank.parents();
            parents.push(head_bank.clone());

            // Ensure the tip of each fork and all its parents are in the given bank_forks
            for parent in parents {
                let parent_bank = &bank_forks[parent.slot()];
                assert_eq!(parent_bank.slot(), parent.slot());
                assert!(parent_bank.is_frozen());
            }
        }
    }
}
