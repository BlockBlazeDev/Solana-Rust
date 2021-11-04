use {
    crate::{config, stake_state::StakeAccount},
    log::*,
    solana_sdk::{
        feature_set,
        instruction::InstructionError,
        keyed_account::{from_keyed_account, get_signers, keyed_account_at_index},
        process_instruction::{get_sysvar, InvokeContext},
        program_utils::limited_deserialize,
        stake::{
            instruction::StakeInstruction,
            program::id,
            state::{Authorized, Lockup},
        },
        sysvar::{self, clock::Clock, rent::Rent, stake_history::StakeHistory},
    },
};

#[deprecated(
    since = "1.8.0",
    note = "Please use `solana_sdk::stake::instruction` or `solana_program::stake::instruction` instead"
)]
pub use solana_sdk::stake::instruction::*;

pub fn process_instruction(
    first_instruction_account: usize,
    data: &[u8],
    invoke_context: &mut dyn InvokeContext,
) -> Result<(), InstructionError> {
    let keyed_accounts = invoke_context.get_keyed_accounts()?;

    trace!("process_instruction: {:?}", data);
    trace!("keyed_accounts: {:?}", keyed_accounts);

    let me = &keyed_account_at_index(keyed_accounts, first_instruction_account)?;
    if me.owner()? != id() {
        return Err(InstructionError::InvalidAccountOwner);
    }

    let signers = get_signers(&keyed_accounts[first_instruction_account..]);
    match limited_deserialize(data)? {
        StakeInstruction::Initialize(authorized, lockup) => me.initialize(
            &authorized,
            &lockup,
            &from_keyed_account::<Rent>(keyed_account_at_index(
                keyed_accounts,
                first_instruction_account + 1,
            )?)?,
        ),
        StakeInstruction::Authorize(authorized_pubkey, stake_authorize) => {
            let require_custodian_for_locked_stake_authorize = invoke_context.is_feature_active(
                &feature_set::require_custodian_for_locked_stake_authorize::id(),
            );

            if require_custodian_for_locked_stake_authorize {
                let clock = from_keyed_account::<Clock>(keyed_account_at_index(
                    keyed_accounts,
                    first_instruction_account + 1,
                )?)?;
                let _current_authority =
                    keyed_account_at_index(keyed_accounts, first_instruction_account + 2)?;
                let custodian =
                    keyed_account_at_index(keyed_accounts, first_instruction_account + 3)
                        .ok()
                        .map(|ka| ka.unsigned_key());

                me.authorize(
                    &signers,
                    &authorized_pubkey,
                    stake_authorize,
                    require_custodian_for_locked_stake_authorize,
                    &clock,
                    custodian,
                )
            } else {
                me.authorize(
                    &signers,
                    &authorized_pubkey,
                    stake_authorize,
                    require_custodian_for_locked_stake_authorize,
                    &Clock::default(),
                    None,
                )
            }
        }
        StakeInstruction::AuthorizeWithSeed(args) => {
            let authority_base =
                keyed_account_at_index(keyed_accounts, first_instruction_account + 1)?;
            let require_custodian_for_locked_stake_authorize = invoke_context.is_feature_active(
                &feature_set::require_custodian_for_locked_stake_authorize::id(),
            );

            if require_custodian_for_locked_stake_authorize {
                let clock = from_keyed_account::<Clock>(keyed_account_at_index(
                    keyed_accounts,
                    first_instruction_account + 2,
                )?)?;
                let custodian =
                    keyed_account_at_index(keyed_accounts, first_instruction_account + 3)
                        .ok()
                        .map(|ka| ka.unsigned_key());

                me.authorize_with_seed(
                    authority_base,
                    &args.authority_seed,
                    &args.authority_owner,
                    &args.new_authorized_pubkey,
                    args.stake_authorize,
                    require_custodian_for_locked_stake_authorize,
                    &clock,
                    custodian,
                )
            } else {
                me.authorize_with_seed(
                    authority_base,
                    &args.authority_seed,
                    &args.authority_owner,
                    &args.new_authorized_pubkey,
                    args.stake_authorize,
                    require_custodian_for_locked_stake_authorize,
                    &Clock::default(),
                    None,
                )
            }
        }
        StakeInstruction::DelegateStake => {
            let can_reverse_deactivation =
                invoke_context.is_feature_active(&feature_set::stake_program_v4::id());
            let vote = keyed_account_at_index(keyed_accounts, first_instruction_account + 1)?;

            me.delegate(
                vote,
                &from_keyed_account::<Clock>(keyed_account_at_index(
                    keyed_accounts,
                    first_instruction_account + 2,
                )?)?,
                &from_keyed_account::<StakeHistory>(keyed_account_at_index(
                    keyed_accounts,
                    first_instruction_account + 3,
                )?)?,
                &config::from_keyed_account(keyed_account_at_index(
                    keyed_accounts,
                    first_instruction_account + 4,
                )?)?,
                &signers,
                can_reverse_deactivation,
            )
        }
        StakeInstruction::Split(lamports) => {
            let split_stake =
                &keyed_account_at_index(keyed_accounts, first_instruction_account + 1)?;
            me.split(lamports, split_stake, &signers)
        }
        StakeInstruction::Merge => {
            let source_stake =
                &keyed_account_at_index(keyed_accounts, first_instruction_account + 1)?;
            let can_merge_expired_lockups =
                invoke_context.is_feature_active(&feature_set::stake_program_v4::id());
            me.merge(
                invoke_context,
                source_stake,
                &from_keyed_account::<Clock>(keyed_account_at_index(
                    keyed_accounts,
                    first_instruction_account + 2,
                )?)?,
                &from_keyed_account::<StakeHistory>(keyed_account_at_index(
                    keyed_accounts,
                    first_instruction_account + 3,
                )?)?,
                &signers,
                can_merge_expired_lockups,
            )
        }
        StakeInstruction::Withdraw(lamports) => {
            let to = &keyed_account_at_index(keyed_accounts, first_instruction_account + 1)?;
            me.withdraw(
                lamports,
                to,
                &from_keyed_account::<Clock>(keyed_account_at_index(
                    keyed_accounts,
                    first_instruction_account + 2,
                )?)?,
                &from_keyed_account::<StakeHistory>(keyed_account_at_index(
                    keyed_accounts,
                    first_instruction_account + 3,
                )?)?,
                keyed_account_at_index(keyed_accounts, first_instruction_account + 4)?,
                keyed_account_at_index(keyed_accounts, first_instruction_account + 5).ok(),
                invoke_context.is_feature_active(&feature_set::stake_program_v4::id()),
            )
        }
        StakeInstruction::Deactivate => me.deactivate(
            &from_keyed_account::<Clock>(keyed_account_at_index(
                keyed_accounts,
                first_instruction_account + 1,
            )?)?,
            &signers,
        ),
        StakeInstruction::SetLockup(lockup) => {
            let clock = if invoke_context.is_feature_active(&feature_set::stake_program_v4::id()) {
                Some(get_sysvar::<Clock>(invoke_context, &sysvar::clock::id())?)
            } else {
                None
            };
            me.set_lockup(&lockup, &signers, clock.as_ref())
        }
        StakeInstruction::InitializeChecked => {
            if invoke_context.is_feature_active(&feature_set::vote_stake_checked_instructions::id())
            {
                let authorized = Authorized {
                    staker: *keyed_account_at_index(keyed_accounts, first_instruction_account + 2)?
                        .unsigned_key(),
                    withdrawer: *keyed_account_at_index(
                        keyed_accounts,
                        first_instruction_account + 3,
                    )?
                    .signer_key()
                    .ok_or(InstructionError::MissingRequiredSignature)?,
                };

                me.initialize(
                    &authorized,
                    &Lockup::default(),
                    &from_keyed_account::<Rent>(keyed_account_at_index(
                        keyed_accounts,
                        first_instruction_account + 1,
                    )?)?,
                )
            } else {
                Err(InstructionError::InvalidInstructionData)
            }
        }
        StakeInstruction::AuthorizeChecked(stake_authorize) => {
            if invoke_context.is_feature_active(&feature_set::vote_stake_checked_instructions::id())
            {
                let clock = from_keyed_account::<Clock>(keyed_account_at_index(
                    keyed_accounts,
                    first_instruction_account + 1,
                )?)?;
                let _current_authority =
                    keyed_account_at_index(keyed_accounts, first_instruction_account + 2)?;
                let authorized_pubkey =
                    &keyed_account_at_index(keyed_accounts, first_instruction_account + 3)?
                        .signer_key()
                        .ok_or(InstructionError::MissingRequiredSignature)?;
                let custodian =
                    keyed_account_at_index(keyed_accounts, first_instruction_account + 4)
                        .ok()
                        .map(|ka| ka.unsigned_key());

                me.authorize(
                    &signers,
                    authorized_pubkey,
                    stake_authorize,
                    true,
                    &clock,
                    custodian,
                )
            } else {
                Err(InstructionError::InvalidInstructionData)
            }
        }
        StakeInstruction::AuthorizeCheckedWithSeed(args) => {
            if invoke_context.is_feature_active(&feature_set::vote_stake_checked_instructions::id())
            {
                let authority_base =
                    keyed_account_at_index(keyed_accounts, first_instruction_account + 1)?;
                let clock = from_keyed_account::<Clock>(keyed_account_at_index(
                    keyed_accounts,
                    first_instruction_account + 2,
                )?)?;
                let authorized_pubkey =
                    &keyed_account_at_index(keyed_accounts, first_instruction_account + 3)?
                        .signer_key()
                        .ok_or(InstructionError::MissingRequiredSignature)?;
                let custodian =
                    keyed_account_at_index(keyed_accounts, first_instruction_account + 4)
                        .ok()
                        .map(|ka| ka.unsigned_key());

                me.authorize_with_seed(
                    authority_base,
                    &args.authority_seed,
                    &args.authority_owner,
                    authorized_pubkey,
                    args.stake_authorize,
                    true,
                    &clock,
                    custodian,
                )
            } else {
                Err(InstructionError::InvalidInstructionData)
            }
        }
        StakeInstruction::SetLockupChecked(lockup_checked) => {
            if invoke_context.is_feature_active(&feature_set::vote_stake_checked_instructions::id())
            {
                let custodian = if let Ok(custodian) =
                    keyed_account_at_index(keyed_accounts, first_instruction_account + 2)
                {
                    Some(
                        *custodian
                            .signer_key()
                            .ok_or(InstructionError::MissingRequiredSignature)?,
                    )
                } else {
                    None
                };

                let lockup = LockupArgs {
                    unix_timestamp: lockup_checked.unix_timestamp,
                    epoch: lockup_checked.epoch,
                    custodian,
                };
                let clock = Some(get_sysvar::<Clock>(invoke_context, &sysvar::clock::id())?);
                me.set_lockup(&lockup, &signers, clock.as_ref())
            } else {
                Err(InstructionError::InvalidInstructionData)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stake_state::{Meta, StakeState};
    use bincode::serialize;
    use solana_sdk::{
        account::{self, Account, AccountSharedData, WritableAccount},
        instruction::{AccountMeta, Instruction},
        keyed_account::create_keyed_accounts_unified,
        process_instruction::MockInvokeContext,
        pubkey::Pubkey,
        rent::Rent,
        stake::{
            config as stake_config,
            instruction::{self, LockupArgs},
            state::{Authorized, Lockup, StakeAuthorize},
        },
        sysvar::{stake_history::StakeHistory, Sysvar},
    };
    use std::{cell::RefCell, str::FromStr};

    fn create_default_account() -> RefCell<AccountSharedData> {
        RefCell::new(AccountSharedData::default())
    }

    fn create_default_stake_account() -> RefCell<AccountSharedData> {
        RefCell::new(AccountSharedData::from(Account {
            owner: id(),
            ..Account::default()
        }))
    }

    fn invalid_stake_state_pubkey() -> Pubkey {
        Pubkey::from_str("BadStake11111111111111111111111111111111111").unwrap()
    }

    fn invalid_vote_state_pubkey() -> Pubkey {
        Pubkey::from_str("BadVote111111111111111111111111111111111111").unwrap()
    }

    fn spoofed_stake_state_pubkey() -> Pubkey {
        Pubkey::from_str("SpoofedStake1111111111111111111111111111111").unwrap()
    }

    fn spoofed_stake_program_id() -> Pubkey {
        Pubkey::from_str("Spoofed111111111111111111111111111111111111").unwrap()
    }

    fn process_instruction(
        owner: &Pubkey,
        instruction_data: &[u8],
        keyed_accounts: &[(bool, bool, &Pubkey, &RefCell<AccountSharedData>)],
    ) -> Result<(), InstructionError> {
        let processor_account = AccountSharedData::new_ref(0, 0, &solana_sdk::native_loader::id());
        let mut keyed_accounts = keyed_accounts.to_vec();
        keyed_accounts.insert(0, (false, false, owner, &processor_account));
        super::process_instruction(
            1,
            instruction_data,
            &mut MockInvokeContext::new(owner, create_keyed_accounts_unified(&keyed_accounts)),
        )
    }

    fn process_instruction_as_one_arg(instruction: &Instruction) -> Result<(), InstructionError> {
        let processor_account = RefCell::new(AccountSharedData::from(Account {
            owner: solana_sdk::native_loader::id(),
            ..Account::default()
        }));
        let accounts: Vec<_> = instruction
            .accounts
            .iter()
            .map(|meta| {
                RefCell::new(if sysvar::clock::check_id(&meta.pubkey) {
                    account::create_account_shared_data_for_test(&sysvar::clock::Clock::default())
                } else if sysvar::rewards::check_id(&meta.pubkey) {
                    account::create_account_shared_data_for_test(&sysvar::rewards::Rewards::new(
                        0.0,
                    ))
                } else if sysvar::stake_history::check_id(&meta.pubkey) {
                    account::create_account_shared_data_for_test(&StakeHistory::default())
                } else if stake_config::check_id(&meta.pubkey) {
                    config::create_account(0, &stake_config::Config::default())
                } else if sysvar::rent::check_id(&meta.pubkey) {
                    account::create_account_shared_data_for_test(&Rent::default())
                } else if meta.pubkey == invalid_stake_state_pubkey() {
                    AccountSharedData::from(Account {
                        owner: id(),
                        ..Account::default()
                    })
                } else if meta.pubkey == invalid_vote_state_pubkey() {
                    AccountSharedData::from(Account {
                        owner: solana_vote_program::id(),
                        ..Account::default()
                    })
                } else if meta.pubkey == spoofed_stake_state_pubkey() {
                    AccountSharedData::from(Account {
                        owner: spoofed_stake_program_id(),
                        ..Account::default()
                    })
                } else {
                    AccountSharedData::from(Account {
                        owner: id(),
                        ..Account::default()
                    })
                })
            })
            .collect();

        {
            let mut keyed_accounts: Vec<_> = instruction
                .accounts
                .iter()
                .zip(accounts.iter())
                .map(|(meta, account)| (meta.is_signer, false, &meta.pubkey, account))
                .collect();
            let processor_id = id();
            keyed_accounts.insert(0, (false, false, &processor_id, &processor_account));
            let mut invoke_context = MockInvokeContext::new(
                &processor_id,
                create_keyed_accounts_unified(&keyed_accounts),
            );
            let mut data = Vec::with_capacity(sysvar::clock::Clock::size_of());
            bincode::serialize_into(&mut data, &sysvar::clock::Clock::default()).unwrap();
            let sysvars = &[(sysvar::clock::id(), data)];
            invoke_context.sysvars = sysvars;
            super::process_instruction(1, &instruction.data, &mut invoke_context)
        }
    }

    #[test]
    fn test_stake_process_instruction() {
        assert_eq!(
            process_instruction_as_one_arg(&instruction::initialize(
                &Pubkey::default(),
                &Authorized::default(),
                &Lockup::default()
            )),
            Err(InstructionError::InvalidAccountData),
        );
        assert_eq!(
            process_instruction_as_one_arg(&instruction::authorize(
                &Pubkey::default(),
                &Pubkey::default(),
                &Pubkey::default(),
                StakeAuthorize::Staker,
                None,
            )),
            Err(InstructionError::InvalidAccountData),
        );
        assert_eq!(
            process_instruction_as_one_arg(
                &instruction::split(
                    &Pubkey::default(),
                    &Pubkey::default(),
                    100,
                    &invalid_stake_state_pubkey(),
                )[2]
            ),
            Err(InstructionError::InvalidAccountData),
        );
        assert_eq!(
            process_instruction_as_one_arg(
                &instruction::merge(
                    &Pubkey::default(),
                    &invalid_stake_state_pubkey(),
                    &Pubkey::default(),
                )[0]
            ),
            Err(InstructionError::InvalidAccountData),
        );
        assert_eq!(
            process_instruction_as_one_arg(
                &instruction::split_with_seed(
                    &Pubkey::default(),
                    &Pubkey::default(),
                    100,
                    &invalid_stake_state_pubkey(),
                    &Pubkey::default(),
                    "seed"
                )[1]
            ),
            Err(InstructionError::InvalidAccountData),
        );
        assert_eq!(
            process_instruction_as_one_arg(&instruction::delegate_stake(
                &Pubkey::default(),
                &Pubkey::default(),
                &invalid_vote_state_pubkey(),
            )),
            Err(InstructionError::InvalidAccountData),
        );
        assert_eq!(
            process_instruction_as_one_arg(&instruction::withdraw(
                &Pubkey::default(),
                &Pubkey::default(),
                &solana_sdk::pubkey::new_rand(),
                100,
                None,
            )),
            Err(InstructionError::InvalidAccountData),
        );
        assert_eq!(
            process_instruction_as_one_arg(&instruction::deactivate_stake(
                &Pubkey::default(),
                &Pubkey::default()
            )),
            Err(InstructionError::InvalidAccountData),
        );
        assert_eq!(
            process_instruction_as_one_arg(&instruction::set_lockup(
                &Pubkey::default(),
                &LockupArgs::default(),
                &Pubkey::default()
            )),
            Err(InstructionError::InvalidAccountData),
        );
    }

    #[test]
    fn test_spoofed_stake_accounts() {
        assert_eq!(
            process_instruction_as_one_arg(&instruction::initialize(
                &spoofed_stake_state_pubkey(),
                &Authorized::default(),
                &Lockup::default()
            )),
            Err(InstructionError::InvalidAccountOwner),
        );
        assert_eq!(
            process_instruction_as_one_arg(&instruction::authorize(
                &spoofed_stake_state_pubkey(),
                &Pubkey::default(),
                &Pubkey::default(),
                StakeAuthorize::Staker,
                None,
            )),
            Err(InstructionError::InvalidAccountOwner),
        );
        assert_eq!(
            process_instruction_as_one_arg(
                &instruction::split(
                    &spoofed_stake_state_pubkey(),
                    &Pubkey::default(),
                    100,
                    &Pubkey::default(),
                )[2]
            ),
            Err(InstructionError::InvalidAccountOwner),
        );
        assert_eq!(
            process_instruction_as_one_arg(
                &instruction::split(
                    &Pubkey::default(),
                    &Pubkey::default(),
                    100,
                    &spoofed_stake_state_pubkey(),
                )[2]
            ),
            Err(InstructionError::IncorrectProgramId),
        );
        assert_eq!(
            process_instruction_as_one_arg(
                &instruction::merge(
                    &spoofed_stake_state_pubkey(),
                    &Pubkey::default(),
                    &Pubkey::default(),
                )[0]
            ),
            Err(InstructionError::InvalidAccountOwner),
        );
        assert_eq!(
            process_instruction_as_one_arg(
                &instruction::merge(
                    &Pubkey::default(),
                    &spoofed_stake_state_pubkey(),
                    &Pubkey::default(),
                )[0]
            ),
            Err(InstructionError::IncorrectProgramId),
        );
        assert_eq!(
            process_instruction_as_one_arg(
                &instruction::split_with_seed(
                    &spoofed_stake_state_pubkey(),
                    &Pubkey::default(),
                    100,
                    &Pubkey::default(),
                    &Pubkey::default(),
                    "seed"
                )[1]
            ),
            Err(InstructionError::InvalidAccountOwner),
        );
        assert_eq!(
            process_instruction_as_one_arg(&instruction::delegate_stake(
                &spoofed_stake_state_pubkey(),
                &Pubkey::default(),
                &Pubkey::default(),
            )),
            Err(InstructionError::InvalidAccountOwner),
        );
        assert_eq!(
            process_instruction_as_one_arg(&instruction::withdraw(
                &spoofed_stake_state_pubkey(),
                &Pubkey::default(),
                &solana_sdk::pubkey::new_rand(),
                100,
                None,
            )),
            Err(InstructionError::InvalidAccountOwner),
        );
        assert_eq!(
            process_instruction_as_one_arg(&instruction::deactivate_stake(
                &spoofed_stake_state_pubkey(),
                &Pubkey::default()
            )),
            Err(InstructionError::InvalidAccountOwner),
        );
        assert_eq!(
            process_instruction_as_one_arg(&instruction::set_lockup(
                &spoofed_stake_state_pubkey(),
                &LockupArgs::default(),
                &Pubkey::default()
            )),
            Err(InstructionError::InvalidAccountOwner),
        );
    }

    #[test]
    fn test_stake_process_instruction_decode_bail() {
        // these will not call stake_state, have bogus contents

        // gets the "is_empty()" check
        assert_eq!(
            process_instruction(
                &Pubkey::default(),
                &serialize(&StakeInstruction::Initialize(
                    Authorized::default(),
                    Lockup::default()
                ))
                .unwrap(),
                &[],
            ),
            Err(InstructionError::NotEnoughAccountKeys),
        );

        // no account for rent
        let stake_address = Pubkey::default();
        let stake_account = create_default_stake_account();
        let keyed_accounts = [(false, false, &stake_address, &stake_account)];
        assert_eq!(
            process_instruction(
                &Pubkey::default(),
                &serialize(&StakeInstruction::Initialize(
                    Authorized::default(),
                    Lockup::default()
                ))
                .unwrap(),
                &keyed_accounts,
            ),
            Err(InstructionError::NotEnoughAccountKeys),
        );

        // rent fails to deserialize
        let stake_address = Pubkey::default();
        let stake_account = create_default_stake_account();
        let rent_address = sysvar::rent::id();
        let rent_account = create_default_account();
        let keyed_accounts = [
            (false, false, &stake_address, &stake_account),
            (false, false, &rent_address, &rent_account),
        ];
        assert_eq!(
            process_instruction(
                &Pubkey::default(),
                &serialize(&StakeInstruction::Initialize(
                    Authorized::default(),
                    Lockup::default()
                ))
                .unwrap(),
                &keyed_accounts,
            ),
            Err(InstructionError::InvalidArgument),
        );

        // fails to deserialize stake state
        let stake_address = Pubkey::default();
        let stake_account = create_default_stake_account();
        let rent_address = sysvar::rent::id();
        let rent_account = RefCell::new(account::create_account_shared_data_for_test(
            &Rent::default(),
        ));
        let keyed_accounts = [
            (false, false, &stake_address, &stake_account),
            (false, false, &rent_address, &rent_account),
        ];
        assert_eq!(
            process_instruction(
                &Pubkey::default(),
                &serialize(&StakeInstruction::Initialize(
                    Authorized::default(),
                    Lockup::default()
                ))
                .unwrap(),
                &keyed_accounts,
            ),
            Err(InstructionError::InvalidAccountData),
        );

        // gets the first check in delegate, wrong number of accounts
        let stake_address = Pubkey::default();
        let stake_account = create_default_stake_account();
        let keyed_accounts = [(false, false, &stake_address, &stake_account)];
        assert_eq!(
            process_instruction(
                &Pubkey::default(),
                &serialize(&StakeInstruction::DelegateStake).unwrap(),
                &keyed_accounts,
            ),
            Err(InstructionError::NotEnoughAccountKeys),
        );

        // gets the sub-check for number of args
        let stake_address = Pubkey::default();
        let stake_account = create_default_stake_account();
        let keyed_accounts = [(false, false, &stake_address, &stake_account)];
        assert_eq!(
            process_instruction(
                &Pubkey::default(),
                &serialize(&StakeInstruction::DelegateStake).unwrap(),
                &keyed_accounts,
            ),
            Err(InstructionError::NotEnoughAccountKeys),
        );

        // gets the check non-deserialize-able account in delegate_stake
        let stake_address = Pubkey::default();
        let stake_account = create_default_stake_account();
        let vote_address = Pubkey::default();
        let mut bad_vote_account = create_default_account();
        bad_vote_account
            .get_mut()
            .set_owner(solana_vote_program::id());
        let clock_address = sysvar::clock::id();
        let clock_account = RefCell::new(account::create_account_shared_data_for_test(
            &sysvar::clock::Clock::default(),
        ));
        let stake_history_address = sysvar::stake_history::id();
        let stake_history_account = RefCell::new(account::create_account_shared_data_for_test(
            &sysvar::stake_history::StakeHistory::default(),
        ));
        let config_address = stake_config::id();
        let config_account =
            RefCell::new(config::create_account(0, &stake_config::Config::default()));
        let keyed_accounts = [
            (true, false, &stake_address, &stake_account),
            (false, false, &vote_address, &bad_vote_account),
            (false, false, &clock_address, &clock_account),
            (false, false, &stake_history_address, &stake_history_account),
            (false, false, &config_address, &config_account),
        ];
        assert_eq!(
            process_instruction(
                &Pubkey::default(),
                &serialize(&StakeInstruction::DelegateStake).unwrap(),
                &keyed_accounts,
            ),
            Err(InstructionError::InvalidAccountData),
        );

        // Tests 3rd keyed account is of correct type (Clock instead of rewards) in withdraw
        let stake_address = Pubkey::default();
        let stake_account = create_default_stake_account();
        let vote_address = Pubkey::default();
        let vote_account = create_default_account();
        let rewards_address = sysvar::rewards::id();
        let rewards_account = RefCell::new(account::create_account_shared_data_for_test(
            &sysvar::rewards::Rewards::new(0.0),
        ));
        let stake_history_address = sysvar::stake_history::id();
        let stake_history_account = RefCell::new(account::create_account_shared_data_for_test(
            &StakeHistory::default(),
        ));
        let keyed_accounts = [
            (false, false, &stake_address, &stake_account),
            (false, false, &vote_address, &vote_account),
            (false, false, &rewards_address, &rewards_account),
            (false, false, &stake_history_address, &stake_history_account),
        ];
        assert_eq!(
            process_instruction(
                &Pubkey::default(),
                &serialize(&StakeInstruction::Withdraw(42)).unwrap(),
                &keyed_accounts,
            ),
            Err(InstructionError::InvalidArgument),
        );

        // Tests correct number of accounts are provided in withdraw
        let stake_address = Pubkey::default();
        let stake_account = create_default_stake_account();
        let keyed_accounts = [(false, false, &stake_address, &stake_account)];
        assert_eq!(
            process_instruction(
                &Pubkey::default(),
                &serialize(&StakeInstruction::Withdraw(42)).unwrap(),
                &keyed_accounts,
            ),
            Err(InstructionError::NotEnoughAccountKeys),
        );

        // Tests 2nd keyed account is of correct type (Clock instead of rewards) in deactivate
        let stake_address = Pubkey::default();
        let stake_account = create_default_stake_account();
        let rewards_address = sysvar::rewards::id();
        let rewards_account = RefCell::new(account::create_account_shared_data_for_test(
            &sysvar::rewards::Rewards::new(0.0),
        ));
        let keyed_accounts = [
            (false, false, &stake_address, &stake_account),
            (false, false, &rewards_address, &rewards_account),
        ];
        assert_eq!(
            process_instruction(
                &Pubkey::default(),
                &serialize(&StakeInstruction::Deactivate).unwrap(),
                &keyed_accounts,
            ),
            Err(InstructionError::InvalidArgument),
        );

        // Tests correct number of accounts are provided in deactivate
        assert_eq!(
            process_instruction(
                &Pubkey::default(),
                &serialize(&StakeInstruction::Deactivate).unwrap(),
                &[],
            ),
            Err(InstructionError::NotEnoughAccountKeys),
        );
    }

    #[test]
    fn test_stake_checked_instructions() {
        let stake_address = Pubkey::new_unique();
        let staker = Pubkey::new_unique();
        let withdrawer = Pubkey::new_unique();

        // Test InitializeChecked with non-signing withdrawer
        let mut instruction =
            initialize_checked(&stake_address, &Authorized { staker, withdrawer });
        instruction.accounts[3] = AccountMeta::new_readonly(withdrawer, false);
        assert_eq!(
            process_instruction_as_one_arg(&instruction),
            Err(InstructionError::MissingRequiredSignature),
        );

        // Test InitializeChecked with withdrawer signer
        let stake_account = AccountSharedData::new_ref(
            1_000_000_000,
            std::mem::size_of::<crate::stake_state::StakeState>(),
            &id(),
        );
        let rent_address = sysvar::rent::id();
        let rent_account = RefCell::new(account::create_account_shared_data_for_test(
            &Rent::default(),
        ));
        let staker_account = create_default_account();
        let withdrawer_account = create_default_account();

        let keyed_accounts: [(bool, bool, &Pubkey, &RefCell<AccountSharedData>); 4] = [
            (false, false, &stake_address, &stake_account),
            (false, false, &rent_address, &rent_account),
            (false, false, &staker, &staker_account),
            (true, false, &withdrawer, &withdrawer_account),
        ];
        assert_eq!(
            process_instruction(
                &Pubkey::default(),
                &serialize(&StakeInstruction::InitializeChecked).unwrap(),
                &keyed_accounts,
            ),
            Ok(()),
        );

        // Test AuthorizeChecked with non-signing authority
        let authorized_address = Pubkey::new_unique();
        let mut instruction = authorize_checked(
            &stake_address,
            &authorized_address,
            &staker,
            StakeAuthorize::Staker,
            None,
        );
        instruction.accounts[3] = AccountMeta::new_readonly(staker, false);
        assert_eq!(
            process_instruction_as_one_arg(&instruction),
            Err(InstructionError::MissingRequiredSignature),
        );

        let mut instruction = authorize_checked(
            &stake_address,
            &authorized_address,
            &withdrawer,
            StakeAuthorize::Withdrawer,
            None,
        );
        instruction.accounts[3] = AccountMeta::new_readonly(withdrawer, false);
        assert_eq!(
            process_instruction_as_one_arg(&instruction),
            Err(InstructionError::MissingRequiredSignature),
        );

        // Test AuthorizeChecked with authority signer
        let stake_account = AccountSharedData::new_ref_data_with_space(
            42,
            &StakeState::Initialized(Meta::auto(&authorized_address)),
            std::mem::size_of::<StakeState>(),
            &id(),
        )
        .unwrap();
        let clock_address = sysvar::clock::id();
        let clock_account = RefCell::new(account::create_account_shared_data_for_test(
            &Clock::default(),
        ));
        let authorized_account = create_default_account();
        let new_authorized_account = create_default_account();

        let mut keyed_accounts = [
            (false, false, &stake_address, &stake_account),
            (false, false, &clock_address, &clock_account),
            (true, false, &authorized_address, &authorized_account),
            (true, false, &staker, &new_authorized_account),
        ];
        assert_eq!(
            process_instruction(
                &Pubkey::default(),
                &serialize(&StakeInstruction::AuthorizeChecked(StakeAuthorize::Staker)).unwrap(),
                &keyed_accounts,
            ),
            Ok(()),
        );

        keyed_accounts[3] = (true, false, &withdrawer, &new_authorized_account);
        assert_eq!(
            process_instruction(
                &Pubkey::default(),
                &serialize(&StakeInstruction::AuthorizeChecked(
                    StakeAuthorize::Withdrawer
                ))
                .unwrap(),
                &keyed_accounts,
            ),
            Ok(()),
        );

        // Test AuthorizeCheckedWithSeed with non-signing authority
        let authorized_owner = Pubkey::new_unique();
        let seed = "test seed";
        let address_with_seed =
            Pubkey::create_with_seed(&authorized_owner, seed, &authorized_owner).unwrap();
        let mut instruction = authorize_checked_with_seed(
            &stake_address,
            &authorized_owner,
            seed.to_string(),
            &authorized_owner,
            &staker,
            StakeAuthorize::Staker,
            None,
        );
        instruction.accounts[3] = AccountMeta::new_readonly(staker, false);
        assert_eq!(
            process_instruction_as_one_arg(&instruction),
            Err(InstructionError::MissingRequiredSignature),
        );

        let mut instruction = authorize_checked_with_seed(
            &stake_address,
            &authorized_owner,
            seed.to_string(),
            &authorized_owner,
            &staker,
            StakeAuthorize::Withdrawer,
            None,
        );
        instruction.accounts[3] = AccountMeta::new_readonly(staker, false);
        assert_eq!(
            process_instruction_as_one_arg(&instruction),
            Err(InstructionError::MissingRequiredSignature),
        );

        // Test AuthorizeCheckedWithSeed with authority signer
        let stake_account = AccountSharedData::new_ref_data_with_space(
            42,
            &StakeState::Initialized(Meta::auto(&address_with_seed)),
            std::mem::size_of::<StakeState>(),
            &id(),
        )
        .unwrap();
        let mut keyed_accounts = [
            (false, false, &address_with_seed, &stake_account),
            (true, false, &authorized_owner, &authorized_account),
            (false, false, &clock_address, &clock_account),
            (true, false, &staker, &new_authorized_account),
        ];
        assert_eq!(
            process_instruction(
                &Pubkey::default(),
                &serialize(&StakeInstruction::AuthorizeCheckedWithSeed(
                    AuthorizeCheckedWithSeedArgs {
                        stake_authorize: StakeAuthorize::Staker,
                        authority_seed: seed.to_string(),
                        authority_owner: authorized_owner,
                    }
                ))
                .unwrap(),
                &keyed_accounts,
            ),
            Ok(()),
        );

        keyed_accounts[3] = (true, false, &withdrawer, &new_authorized_account);
        assert_eq!(
            process_instruction(
                &Pubkey::default(),
                &serialize(&StakeInstruction::AuthorizeCheckedWithSeed(
                    AuthorizeCheckedWithSeedArgs {
                        stake_authorize: StakeAuthorize::Withdrawer,
                        authority_seed: seed.to_string(),
                        authority_owner: authorized_owner,
                    }
                ))
                .unwrap(),
                &keyed_accounts,
            ),
            Ok(()),
        );

        // Test SetLockupChecked with non-signing lockup custodian
        let custodian = Pubkey::new_unique();
        let mut instruction = set_lockup_checked(
            &stake_address,
            &LockupArgs {
                unix_timestamp: None,
                epoch: Some(1),
                custodian: Some(custodian),
            },
            &withdrawer,
        );
        instruction.accounts[2] = AccountMeta::new_readonly(custodian, false);
        assert_eq!(
            process_instruction_as_one_arg(&instruction),
            Err(InstructionError::MissingRequiredSignature),
        );

        // Test SetLockupChecked with lockup custodian signer
        let stake_account = AccountSharedData::new_ref_data_with_space(
            42,
            &StakeState::Initialized(Meta::auto(&withdrawer)),
            std::mem::size_of::<StakeState>(),
            &id(),
        )
        .unwrap();
        let custodian_account = create_default_account();

        let processor_account = RefCell::new(AccountSharedData::from(Account {
            owner: solana_sdk::native_loader::id(),
            ..Account::default()
        }));
        let keyed_accounts = [
            (false, false, &id(), &processor_account),
            (false, false, &stake_address, &stake_account),
            (true, false, &withdrawer, &withdrawer_account),
            (true, false, &custodian, &custodian_account),
        ];
        let mut invoke_context =
            MockInvokeContext::new(&id(), create_keyed_accounts_unified(&keyed_accounts));
        let mut data = Vec::with_capacity(sysvar::clock::Clock::size_of());
        bincode::serialize_into(&mut data, &sysvar::clock::Clock::default()).unwrap();
        let sysvars = &[(sysvar::clock::id(), data)];
        invoke_context.sysvars = sysvars;

        assert_eq!(
            super::process_instruction(
                1,
                &serialize(&StakeInstruction::SetLockupChecked(LockupCheckedArgs {
                    unix_timestamp: None,
                    epoch: Some(1),
                }))
                .unwrap(),
                &mut invoke_context,
            ),
            Ok(()),
        );
    }
}
