//! storage program
//!  Receive mining proofs from miners, validate the answers
//!  and give reward for good proofs.

use crate::*;
use log::*;
use solana_sdk::account::KeyedAccount;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::transaction::InstructionError;

pub const TOTAL_VALIDATOR_REWARDS: u64 = 1000;
pub const TOTAL_REPLICATOR_REWARDS: u64 = 1000;

fn count_valid_proofs(proofs: &[ProofStatus]) -> u64 {
    let mut num = 0;
    for proof in proofs {
        if let ProofStatus::Valid = proof {
            num += 1;
        }
    }
    num
}

pub fn process_instruction(
    _program_id: &Pubkey,
    keyed_accounts: &mut [KeyedAccount],
    data: &[u8],
    _tick_height: u64,
) -> Result<(), InstructionError> {
    solana_logger::setup();

    if keyed_accounts.len() != 1 {
        // keyed_accounts[1] should be the main storage key
        // to access its data
        Err(InstructionError::InvalidArgument)?;
    }

    // accounts_keys[0] must be signed
    if keyed_accounts[0].signer_key().is_none() {
        info!("account[0] is unsigned");
        Err(InstructionError::GenericError)?;
    }

    if let Ok(syscall) = bincode::deserialize(data) {
        let mut storage_account_state = if let Ok(storage_account_state) =
            bincode::deserialize(&keyed_accounts[0].account.data)
        {
            storage_account_state
        } else {
            StorageProgramState::default()
        };

        debug!(
            "deserialized state height: {}",
            storage_account_state.entry_height
        );
        match syscall {
            StorageProgram::SubmitMiningProof {
                sha_state,
                entry_height,
                signature,
            } => {
                let segment_index = get_segment_from_entry(entry_height);
                let current_segment_index =
                    get_segment_from_entry(storage_account_state.entry_height);
                if segment_index >= current_segment_index {
                    return Err(InstructionError::InvalidArgument);
                }

                debug!(
                    "Mining proof submitted with state {:?} entry_height: {}",
                    sha_state, entry_height
                );

                let proof_info = ProofInfo {
                    id: *keyed_accounts[0].signer_key().unwrap(),
                    sha_state,
                    signature,
                };
                storage_account_state.proofs[segment_index].push(proof_info);
            }
            StorageProgram::AdvertiseStorageRecentBlockhash { hash, entry_height } => {
                let original_segments = storage_account_state.entry_height / ENTRIES_PER_SEGMENT;
                let segments = entry_height / ENTRIES_PER_SEGMENT;
                debug!(
                    "advertise new last id segments: {} orig: {}",
                    segments, original_segments
                );
                if segments <= original_segments {
                    return Err(InstructionError::InvalidArgument);
                }

                storage_account_state.entry_height = entry_height;
                storage_account_state.hash = hash;

                // move the proofs to previous_proofs
                storage_account_state.previous_proofs = storage_account_state.proofs.clone();
                storage_account_state.proofs.clear();
                storage_account_state
                    .proofs
                    .resize(segments as usize, Vec::new());

                // move lockout_validations to reward_validations
                storage_account_state.reward_validations =
                    storage_account_state.lockout_validations.clone();
                storage_account_state.lockout_validations.clear();
                storage_account_state
                    .lockout_validations
                    .resize(segments as usize, Vec::new());
            }
            StorageProgram::ProofValidation {
                entry_height,
                proof_mask,
            } => {
                if entry_height >= storage_account_state.entry_height {
                    return Err(InstructionError::InvalidArgument);
                }

                let segment_index = get_segment_from_entry(entry_height);
                if storage_account_state.previous_proofs[segment_index].len() != proof_mask.len() {
                    return Err(InstructionError::InvalidArgument);
                }

                // TODO: Check that each proof mask matches the signature
                /*for (i, entry) in proof_mask.iter().enumerate() {
                    if storage_account_state.previous_proofs[segment_index][i] != signature.as_ref[0] {
                        return Err(InstructionError::InvalidArgument);
                    }
                }*/

                let info = ValidationInfo {
                    id: *keyed_accounts[0].signer_key().unwrap(),
                    proof_mask,
                };
                storage_account_state.lockout_validations[segment_index].push(info);
            }
            StorageProgram::ClaimStorageReward { entry_height } => {
                let claims_index = get_segment_from_entry(entry_height);
                let account_key = keyed_accounts[0].signer_key().unwrap();
                let mut num_validations = 0;
                let mut total_validations = 0;
                for validation in &storage_account_state.reward_validations[claims_index] {
                    if *account_key == validation.id {
                        num_validations += count_valid_proofs(&validation.proof_mask);
                    } else {
                        total_validations += count_valid_proofs(&validation.proof_mask);
                    }
                }
                total_validations += num_validations;
                if total_validations > 0 {
                    keyed_accounts[0].account.lamports +=
                        (TOTAL_VALIDATOR_REWARDS * num_validations) / total_validations;
                }
            }
        }

        if bincode::serialize_into(
            &mut keyed_accounts[0].account.data[..],
            &storage_account_state,
        )
        .is_err()
        {
            return Err(InstructionError::AccountDataTooSmall);
        }

        Ok(())
    } else {
        info!("Invalid instruction data: {:?}", data);
        Err(InstructionError::InvalidInstructionData)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ProofStatus, StorageTransaction, ENTRIES_PER_SEGMENT};
    use bincode::deserialize;
    use solana_runtime::bank::Bank;
    use solana_sdk::account::{create_keyed_accounts, Account};
    use solana_sdk::genesis_block::GenesisBlock;
    use solana_sdk::hash::{hash, Hash};
    use solana_sdk::pubkey::Pubkey;
    use solana_sdk::signature::{Keypair, KeypairUtil, Signature};
    use solana_sdk::system_transaction::SystemTransaction;
    use solana_sdk::transaction::{CompiledInstruction, Transaction};

    fn test_transaction(
        tx: &Transaction,
        program_accounts: &mut [Account],
    ) -> Result<(), InstructionError> {
        assert_eq!(tx.instructions.len(), 1);
        let CompiledInstruction {
            ref accounts,
            ref data,
            ..
        } = tx.instructions[0];

        info!("accounts: {:?}", accounts);

        let mut keyed_accounts: Vec<_> = accounts
            .iter()
            .map(|&index| {
                let index = index as usize;
                let key = &tx.account_keys[index];
                (key, index < tx.signatures.len())
            })
            .zip(program_accounts.iter_mut())
            .map(|((key, is_signer), account)| KeyedAccount::new(key, is_signer, account))
            .collect();

        let ret = process_instruction(&id(), &mut keyed_accounts, &data, 42);
        info!("ret: {:?}", ret);
        ret
    }

    #[test]
    fn test_storage_tx() {
        let keypair = Keypair::new();
        let mut accounts = [(keypair.pubkey(), Account::default())];
        let mut keyed_accounts = create_keyed_accounts(&mut accounts);
        assert!(process_instruction(&id(), &mut keyed_accounts, &[], 42).is_err());
    }

    #[test]
    fn test_serialize_overflow() {
        let keypair = Keypair::new();
        let mut keyed_accounts = Vec::new();
        let mut user_account = Account::default();
        let pubkey = keypair.pubkey();
        keyed_accounts.push(KeyedAccount::new(&pubkey, true, &mut user_account));

        let tx = StorageTransaction::new_advertise_recent_blockhash(
            &keypair,
            Hash::default(),
            Hash::default(),
            ENTRIES_PER_SEGMENT,
        );

        assert_eq!(
            process_instruction(&id(), &mut keyed_accounts, &tx.instructions[0].data, 42),
            Err(InstructionError::AccountDataTooSmall)
        );
    }

    #[test]
    fn test_invalid_accounts_len() {
        let keypair = Keypair::new();
        let mut accounts = [Account::default()];

        let tx = StorageTransaction::new_mining_proof(
            &keypair,
            Hash::default(),
            Hash::default(),
            0,
            Signature::default(),
        );
        assert!(test_transaction(&tx, &mut accounts).is_err());

        let mut accounts = [Account::default(), Account::default(), Account::default()];

        assert!(test_transaction(&tx, &mut accounts).is_err());
    }

    #[test]
    fn test_submit_mining_invalid_entry_height() {
        solana_logger::setup();
        let keypair = Keypair::new();
        let mut accounts = [Account::default(), Account::default()];
        accounts[1].data.resize(16 * 1024, 0);

        let tx = StorageTransaction::new_mining_proof(
            &keypair,
            Hash::default(),
            Hash::default(),
            0,
            Signature::default(),
        );

        // Haven't seen a transaction to roll over the epoch, so this should fail
        assert!(test_transaction(&tx, &mut accounts).is_err());
    }

    #[test]
    fn test_submit_mining_ok() {
        solana_logger::setup();
        let keypair = Keypair::new();
        let mut accounts = [Account::default(), Account::default()];
        accounts[0].data.resize(16 * 1024, 0);

        let tx = StorageTransaction::new_advertise_recent_blockhash(
            &keypair,
            Hash::default(),
            Hash::default(),
            ENTRIES_PER_SEGMENT,
        );

        test_transaction(&tx, &mut accounts).unwrap();

        let tx = StorageTransaction::new_mining_proof(
            &keypair,
            Hash::default(),
            Hash::default(),
            0,
            Signature::default(),
        );

        test_transaction(&tx, &mut accounts).unwrap();
    }

    #[test]
    fn test_validate_mining() {
        solana_logger::setup();
        let keypair = Keypair::new();
        let mut accounts = [Account::default(), Account::default()];
        accounts[0].data.resize(16 * 1024, 0);

        let entry_height = 0;

        let tx = StorageTransaction::new_advertise_recent_blockhash(
            &keypair,
            Hash::default(),
            Hash::default(),
            ENTRIES_PER_SEGMENT,
        );

        test_transaction(&tx, &mut accounts).unwrap();

        let tx = StorageTransaction::new_mining_proof(
            &keypair,
            Hash::default(),
            Hash::default(),
            entry_height,
            Signature::default(),
        );
        test_transaction(&tx, &mut accounts).unwrap();

        let tx = StorageTransaction::new_advertise_recent_blockhash(
            &keypair,
            Hash::default(),
            Hash::default(),
            ENTRIES_PER_SEGMENT * 2,
        );
        test_transaction(&tx, &mut accounts).unwrap();

        let tx = StorageTransaction::new_proof_validation(
            &keypair,
            Hash::default(),
            entry_height,
            vec![ProofStatus::Valid],
        );
        test_transaction(&tx, &mut accounts).unwrap();

        let tx = StorageTransaction::new_advertise_recent_blockhash(
            &keypair,
            Hash::default(),
            Hash::default(),
            ENTRIES_PER_SEGMENT * 3,
        );
        test_transaction(&tx, &mut accounts).unwrap();

        let tx = StorageTransaction::new_reward_claim(&keypair, Hash::default(), entry_height);
        test_transaction(&tx, &mut accounts).unwrap();

        assert!(accounts[0].lamports == TOTAL_VALIDATOR_REWARDS);
    }

    fn get_storage_entry_height(bank: &Bank, account: &Pubkey) -> u64 {
        match bank.get_account(&account) {
            Some(storage_system_account) => {
                let state = deserialize(&storage_system_account.data);
                if let Ok(state) = state {
                    let state: StorageProgramState = state;
                    return state.entry_height;
                }
            }
            None => {
                info!("error in reading entry_height");
            }
        }
        0
    }

    fn get_storage_blockhash(bank: &Bank, account: &Pubkey) -> Hash {
        if let Some(storage_system_account) = bank.get_account(&account) {
            let state = deserialize(&storage_system_account.data);
            if let Ok(state) = state {
                let state: StorageProgramState = state;
                return state.hash;
            }
        }
        Hash::default()
    }

    #[test]
    fn test_bank_storage() {
        let (mut genesis_block, alice) = GenesisBlock::new(1000);
        genesis_block
            .native_programs
            .push(("solana_storage_program".to_string(), id()));
        let bank = Bank::new(&genesis_block);

        let bob = Keypair::new();
        let jack = Keypair::new();
        let jill = Keypair::new();

        let x = 42;
        let blockhash = genesis_block.hash();
        let x2 = x * 2;
        let storage_blockhash = hash(&[x2]);

        bank.register_tick(&blockhash);

        bank.transfer(10, &alice, &jill.pubkey(), blockhash)
            .unwrap();

        bank.transfer(10, &alice, &bob.pubkey(), blockhash).unwrap();
        bank.transfer(10, &alice, &jack.pubkey(), blockhash)
            .unwrap();

        let tx = SystemTransaction::new_program_account(
            &alice,
            &bob.pubkey(),
            blockhash,
            1,
            4 * 1024,
            &id(),
            0,
        );

        bank.process_transaction(&tx).unwrap();

        let tx = StorageTransaction::new_advertise_recent_blockhash(
            &bob,
            storage_blockhash,
            blockhash,
            ENTRIES_PER_SEGMENT,
        );

        bank.process_transaction(&tx).unwrap();

        let entry_height = 0;
        let tx = StorageTransaction::new_mining_proof(
            &bob,
            Hash::default(),
            blockhash,
            entry_height,
            Signature::default(),
        );
        let _result = bank.process_transaction(&tx).unwrap();

        assert_eq!(
            get_storage_entry_height(&bank, &bob.pubkey()),
            ENTRIES_PER_SEGMENT
        );
        assert_eq!(
            get_storage_blockhash(&bank, &bob.pubkey()),
            storage_blockhash
        );
    }
}
