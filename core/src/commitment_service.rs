use crate::{
    consensus::Stake,
    rpc_subscriptions::{CacheSlotInfo, RpcSubscriptions},
};
use solana_measure::measure::Measure;
use solana_metrics::datapoint_info;
use solana_runtime::{
    bank::Bank,
    commitment::{BlockCommitment, BlockCommitmentCache, VOTE_THRESHOLD_SIZE},
};
use solana_sdk::clock::Slot;
use solana_vote_program::vote_state::VoteState;
use std::{
    collections::HashMap,
    sync::atomic::{AtomicBool, Ordering},
    sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender},
    sync::{Arc, RwLock},
    thread::{self, Builder, JoinHandle},
    time::Duration,
};

pub struct CommitmentAggregationData {
    bank: Arc<Bank>,
    root: Slot,
    total_stake: Stake,
}

impl CommitmentAggregationData {
    pub fn new(bank: Arc<Bank>, root: Slot, total_stake: Stake) -> Self {
        Self {
            bank,
            root,
            total_stake,
        }
    }
}

fn get_highest_confirmed_root(mut rooted_stake: Vec<(Slot, u64)>, total_stake: u64) -> Slot {
    rooted_stake.sort_by(|a, b| a.0.cmp(&b.0).reverse());
    let mut stake_sum = 0;
    for (root, stake) in rooted_stake {
        stake_sum += stake;
        if (stake_sum as f64 / total_stake as f64) > VOTE_THRESHOLD_SIZE {
            return root;
        }
    }
    0
}

pub struct AggregateCommitmentService {
    t_commitment: JoinHandle<()>,
}

impl AggregateCommitmentService {
    pub fn new(
        exit: &Arc<AtomicBool>,
        block_commitment_cache: Arc<RwLock<BlockCommitmentCache>>,
        subscriptions: Arc<RpcSubscriptions>,
    ) -> (Sender<CommitmentAggregationData>, Self) {
        let (sender, receiver): (
            Sender<CommitmentAggregationData>,
            Receiver<CommitmentAggregationData>,
        ) = channel();
        let exit_ = exit.clone();
        (
            sender,
            Self {
                t_commitment: Builder::new()
                    .name("solana-aggregate-stake-lockouts".to_string())
                    .spawn(move || loop {
                        if exit_.load(Ordering::Relaxed) {
                            break;
                        }

                        if let Err(RecvTimeoutError::Disconnected) =
                            Self::run(&receiver, &block_commitment_cache, &subscriptions, &exit_)
                        {
                            break;
                        }
                    })
                    .unwrap(),
            },
        )
    }

    fn run(
        receiver: &Receiver<CommitmentAggregationData>,
        block_commitment_cache: &RwLock<BlockCommitmentCache>,
        subscriptions: &Arc<RpcSubscriptions>,
        exit: &Arc<AtomicBool>,
    ) -> Result<(), RecvTimeoutError> {
        loop {
            if exit.load(Ordering::Relaxed) {
                return Ok(());
            }

            let mut aggregation_data = receiver.recv_timeout(Duration::from_secs(1))?;

            while let Ok(new_data) = receiver.try_recv() {
                aggregation_data = new_data;
            }

            let ancestors = aggregation_data.bank.status_cache_ancestors();
            if ancestors.is_empty() {
                continue;
            }

            let mut aggregate_commitment_time = Measure::start("aggregate-commitment-ms");
            let (block_commitment, rooted_stake) =
                Self::aggregate_commitment(&ancestors, &aggregation_data.bank);

            let highest_confirmed_root =
                get_highest_confirmed_root(rooted_stake, aggregation_data.total_stake);

            let mut new_block_commitment = BlockCommitmentCache::new(
                block_commitment,
                highest_confirmed_root,
                aggregation_data.total_stake,
                aggregation_data.bank,
                aggregation_data.root,
                aggregation_data.root,
            );
            new_block_commitment.highest_confirmed_slot =
                new_block_commitment.calculate_highest_confirmed_slot();

            let mut w_block_commitment_cache = block_commitment_cache.write().unwrap();

            std::mem::swap(&mut *w_block_commitment_cache, &mut new_block_commitment);
            aggregate_commitment_time.stop();
            datapoint_info!(
                "block-commitment-cache",
                (
                    "aggregate-commitment-ms",
                    aggregate_commitment_time.as_ms() as i64,
                    i64
                )
            );

            subscriptions.notify_subscribers(CacheSlotInfo {
                current_slot: w_block_commitment_cache.slot(),
                node_root: w_block_commitment_cache.root(),
                highest_confirmed_root: w_block_commitment_cache.highest_confirmed_root(),
                highest_confirmed_slot: w_block_commitment_cache.highest_confirmed_slot(),
            });
        }
    }

    pub fn aggregate_commitment(
        ancestors: &[Slot],
        bank: &Bank,
    ) -> (HashMap<Slot, BlockCommitment>, Vec<(Slot, u64)>) {
        assert!(!ancestors.is_empty());

        // Check ancestors is sorted
        for a in ancestors.windows(2) {
            assert!(a[0] < a[1]);
        }

        let mut commitment = HashMap::new();
        let mut rooted_stake: Vec<(Slot, u64)> = Vec::new();
        for (_, (lamports, account)) in bank.vote_accounts().into_iter() {
            if lamports == 0 {
                continue;
            }
            let vote_state = VoteState::from(&account);
            if vote_state.is_none() {
                continue;
            }

            let vote_state = vote_state.unwrap();
            Self::aggregate_commitment_for_vote_account(
                &mut commitment,
                &mut rooted_stake,
                &vote_state,
                ancestors,
                lamports,
            );
        }

        (commitment, rooted_stake)
    }

    fn aggregate_commitment_for_vote_account(
        commitment: &mut HashMap<Slot, BlockCommitment>,
        rooted_stake: &mut Vec<(Slot, u64)>,
        vote_state: &VoteState,
        ancestors: &[Slot],
        lamports: u64,
    ) {
        assert!(!ancestors.is_empty());
        let mut ancestors_index = 0;
        if let Some(root) = vote_state.root_slot {
            for (i, a) in ancestors.iter().enumerate() {
                if *a <= root {
                    commitment
                        .entry(*a)
                        .or_insert_with(BlockCommitment::default)
                        .increase_rooted_stake(lamports);
                } else {
                    ancestors_index = i;
                    break;
                }
            }
            rooted_stake.push((root, lamports));
        }

        for vote in &vote_state.votes {
            while ancestors[ancestors_index] <= vote.slot {
                commitment
                    .entry(ancestors[ancestors_index])
                    .or_insert_with(BlockCommitment::default)
                    .increase_confirmation_stake(vote.confirmation_count as usize, lamports);
                ancestors_index += 1;

                if ancestors_index == ancestors.len() {
                    return;
                }
            }
        }
    }

    pub fn join(self) -> thread::Result<()> {
        self.t_commitment.join()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_ledger::genesis_utils::{create_genesis_config, GenesisConfigInfo};
    use solana_sdk::pubkey::Pubkey;
    use solana_stake_program::stake_state;
    use solana_vote_program::vote_state::{self, VoteStateVersions};

    #[test]
    fn test_get_highest_confirmed_root() {
        assert_eq!(get_highest_confirmed_root(vec![], 10), 0);
        let mut rooted_stake = vec![];
        rooted_stake.push((0, 5));
        rooted_stake.push((1, 5));
        assert_eq!(get_highest_confirmed_root(rooted_stake, 10), 0);
        let mut rooted_stake = vec![];
        rooted_stake.push((1, 5));
        rooted_stake.push((0, 10));
        rooted_stake.push((2, 5));
        rooted_stake.push((1, 4));
        assert_eq!(get_highest_confirmed_root(rooted_stake, 10), 1);
    }

    #[test]
    fn test_aggregate_commitment_for_vote_account_1() {
        let ancestors = vec![3, 4, 5, 7, 9, 11];
        let mut commitment = HashMap::new();
        let mut rooted_stake = vec![];
        let lamports = 5;
        let mut vote_state = VoteState::default();

        let root = *ancestors.last().unwrap();
        vote_state.root_slot = Some(root);
        AggregateCommitmentService::aggregate_commitment_for_vote_account(
            &mut commitment,
            &mut rooted_stake,
            &vote_state,
            &ancestors,
            lamports,
        );

        for a in ancestors {
            let mut expected = BlockCommitment::default();
            expected.increase_rooted_stake(lamports);
            assert_eq!(*commitment.get(&a).unwrap(), expected);
        }
        assert_eq!(rooted_stake[0], (root, lamports));
    }

    #[test]
    fn test_aggregate_commitment_for_vote_account_2() {
        let ancestors = vec![3, 4, 5, 7, 9, 11];
        let mut commitment = HashMap::new();
        let mut rooted_stake = vec![];
        let lamports = 5;
        let mut vote_state = VoteState::default();

        let root = ancestors[2];
        vote_state.root_slot = Some(root);
        vote_state.process_slot_vote_unchecked(*ancestors.last().unwrap());
        AggregateCommitmentService::aggregate_commitment_for_vote_account(
            &mut commitment,
            &mut rooted_stake,
            &vote_state,
            &ancestors,
            lamports,
        );

        for a in ancestors {
            if a <= root {
                let mut expected = BlockCommitment::default();
                expected.increase_rooted_stake(lamports);
                assert_eq!(*commitment.get(&a).unwrap(), expected);
            } else {
                let mut expected = BlockCommitment::default();
                expected.increase_confirmation_stake(1, lamports);
                assert_eq!(*commitment.get(&a).unwrap(), expected);
            }
        }
        assert_eq!(rooted_stake[0], (root, lamports));
    }

    #[test]
    fn test_aggregate_commitment_for_vote_account_3() {
        let ancestors = vec![3, 4, 5, 7, 9, 10, 11];
        let mut commitment = HashMap::new();
        let mut rooted_stake = vec![];
        let lamports = 5;
        let mut vote_state = VoteState::default();

        let root = ancestors[2];
        vote_state.root_slot = Some(root);
        assert!(ancestors[4] + 2 >= ancestors[6]);
        vote_state.process_slot_vote_unchecked(ancestors[4]);
        vote_state.process_slot_vote_unchecked(ancestors[6]);
        AggregateCommitmentService::aggregate_commitment_for_vote_account(
            &mut commitment,
            &mut rooted_stake,
            &vote_state,
            &ancestors,
            lamports,
        );

        for (i, a) in ancestors.iter().enumerate() {
            if *a <= root {
                let mut expected = BlockCommitment::default();
                expected.increase_rooted_stake(lamports);
                assert_eq!(*commitment.get(&a).unwrap(), expected);
            } else if i <= 4 {
                let mut expected = BlockCommitment::default();
                expected.increase_confirmation_stake(2, lamports);
                assert_eq!(*commitment.get(&a).unwrap(), expected);
            } else if i <= 6 {
                let mut expected = BlockCommitment::default();
                expected.increase_confirmation_stake(1, lamports);
                assert_eq!(*commitment.get(&a).unwrap(), expected);
            }
        }
        assert_eq!(rooted_stake[0], (root, lamports));
    }

    #[test]
    fn test_aggregate_commitment_validity() {
        let ancestors = vec![3, 4, 5, 7, 9, 10, 11];
        let GenesisConfigInfo {
            mut genesis_config, ..
        } = create_genesis_config(10_000);

        let rooted_stake_amount = 40;

        let sk1 = Pubkey::new_rand();
        let pk1 = Pubkey::new_rand();
        let mut vote_account1 = vote_state::create_account(&pk1, &Pubkey::new_rand(), 0, 100);
        let stake_account1 =
            stake_state::create_account(&sk1, &pk1, &vote_account1, &genesis_config.rent, 100);
        let sk2 = Pubkey::new_rand();
        let pk2 = Pubkey::new_rand();
        let mut vote_account2 = vote_state::create_account(&pk2, &Pubkey::new_rand(), 0, 50);
        let stake_account2 =
            stake_state::create_account(&sk2, &pk2, &vote_account2, &genesis_config.rent, 50);
        let sk3 = Pubkey::new_rand();
        let pk3 = Pubkey::new_rand();
        let mut vote_account3 = vote_state::create_account(&pk3, &Pubkey::new_rand(), 0, 1);
        let stake_account3 = stake_state::create_account(
            &sk3,
            &pk3,
            &vote_account3,
            &genesis_config.rent,
            rooted_stake_amount,
        );
        let sk4 = Pubkey::new_rand();
        let pk4 = Pubkey::new_rand();
        let mut vote_account4 = vote_state::create_account(&pk4, &Pubkey::new_rand(), 0, 1);
        let stake_account4 = stake_state::create_account(
            &sk4,
            &pk4,
            &vote_account4,
            &genesis_config.rent,
            rooted_stake_amount,
        );

        genesis_config.accounts.extend(vec![
            (pk1, vote_account1.clone()),
            (sk1, stake_account1),
            (pk2, vote_account2.clone()),
            (sk2, stake_account2),
            (pk3, vote_account3.clone()),
            (sk3, stake_account3),
            (pk4, vote_account4.clone()),
            (sk4, stake_account4),
        ]);

        // Create bank
        let bank = Arc::new(Bank::new(&genesis_config));

        let mut vote_state1 = VoteState::from(&vote_account1).unwrap();
        vote_state1.process_slot_vote_unchecked(3);
        vote_state1.process_slot_vote_unchecked(5);
        let versioned = VoteStateVersions::Current(Box::new(vote_state1));
        VoteState::to(&versioned, &mut vote_account1).unwrap();
        bank.store_account(&pk1, &vote_account1);

        let mut vote_state2 = VoteState::from(&vote_account2).unwrap();
        vote_state2.process_slot_vote_unchecked(9);
        vote_state2.process_slot_vote_unchecked(10);
        let versioned = VoteStateVersions::Current(Box::new(vote_state2));
        VoteState::to(&versioned, &mut vote_account2).unwrap();
        bank.store_account(&pk2, &vote_account2);

        let mut vote_state3 = VoteState::from(&vote_account3).unwrap();
        vote_state3.root_slot = Some(1);
        let versioned = VoteStateVersions::Current(Box::new(vote_state3));
        VoteState::to(&versioned, &mut vote_account3).unwrap();
        bank.store_account(&pk3, &vote_account3);

        let mut vote_state4 = VoteState::from(&vote_account4).unwrap();
        vote_state4.root_slot = Some(2);
        let versioned = VoteStateVersions::Current(Box::new(vote_state4));
        VoteState::to(&versioned, &mut vote_account4).unwrap();
        bank.store_account(&pk4, &vote_account4);

        let (commitment, rooted_stake) =
            AggregateCommitmentService::aggregate_commitment(&ancestors, &bank);

        for a in ancestors {
            if a <= 3 {
                let mut expected = BlockCommitment::default();
                expected.increase_confirmation_stake(2, 150);
                assert_eq!(*commitment.get(&a).unwrap(), expected);
            } else if a <= 5 {
                let mut expected = BlockCommitment::default();
                expected.increase_confirmation_stake(1, 100);
                expected.increase_confirmation_stake(2, 50);
                assert_eq!(*commitment.get(&a).unwrap(), expected);
            } else if a <= 9 {
                let mut expected = BlockCommitment::default();
                expected.increase_confirmation_stake(2, 50);
                assert_eq!(*commitment.get(&a).unwrap(), expected);
            } else if a <= 10 {
                let mut expected = BlockCommitment::default();
                expected.increase_confirmation_stake(1, 50);
                assert_eq!(*commitment.get(&a).unwrap(), expected);
            } else {
                assert!(commitment.get(&a).is_none());
            }
        }
        assert_eq!(rooted_stake.len(), 2);
        assert_eq!(get_highest_confirmed_root(rooted_stake, 100), 1)
    }
}
