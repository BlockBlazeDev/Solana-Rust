use {
    crate::id,
    serde::{Deserialize, Serialize},
    solana_sdk::{
        clock::Slot,
        instruction::{AccountMeta, Instruction},
        pubkey::Pubkey,
        system_program,
    },
};

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub enum ProgramInstruction {
    /// Create an address lookup table
    ///
    /// # Account references
    ///   0. `[WRITE]` Uninitialized address lookup table account
    ///   1. `[SIGNER]` Account used to derive and control the new address lookup table.
    ///   2. `[SIGNER, WRITE]` Account that will fund the new address lookup table.
    ///   3. `[]` System program for CPI.
    CreateLookupTable {
        /// A recent slot must be used in the derivation path
        /// for each initialized table. When closing table accounts,
        /// the initialization slot must no longer be "recent" to prevent
        /// address tables from being recreated with reordered or
        /// otherwise malicious addresses.
        recent_slot: Slot,
        /// Address tables are always initialized at program-derived
        /// addresses using the funding address, recent blockhash, and
        /// the user-passed `bump_seed`.
        bump_seed: u8,
    },

    /// Permanently freeze a address lookup table, making it immutable.
    ///
    /// # Account references
    ///   0. `[WRITE]` Address lookup table account to freeze
    ///   1. `[SIGNER]` Current authority
    FreezeLookupTable,

    /// Extend an address lookup table with new addresses
    ///
    /// # Account references
    ///   0. `[WRITE]` Address lookup table account to extend
    ///   1. `[SIGNER]` Current authority
    ///   2. `[SIGNER, WRITE]` Account that will fund the table reallocation
    ///   3. `[]` System program for CPI.
    ExtendLookupTable { new_addresses: Vec<Pubkey> },

    /// Close an address lookup table account
    ///
    /// # Account references
    ///   0. `[WRITE]` Address lookup table account to close
    ///   1. `[SIGNER]` Current authority
    ///   2. `[WRITE]` Recipient of closed account lamports
    CloseLookupTable,
}

/// Derives the address of an address table account from a wallet address and a recent block's slot.
pub fn derive_lookup_table_address(
    authority_address: &Pubkey,
    recent_block_slot: Slot,
) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[authority_address.as_ref(), &recent_block_slot.to_le_bytes()],
        &id(),
    )
}

/// Constructs an instruction to create a table account and returns
/// the instruction and the table account's derived address.
pub fn create_lookup_table(
    authority_address: Pubkey,
    payer_address: Pubkey,
    recent_slot: Slot,
) -> (Instruction, Pubkey) {
    let (lookup_table_address, bump_seed) =
        derive_lookup_table_address(&authority_address, recent_slot);
    let instruction = Instruction::new_with_bincode(
        id(),
        &ProgramInstruction::CreateLookupTable {
            recent_slot,
            bump_seed,
        },
        vec![
            AccountMeta::new(lookup_table_address, false),
            AccountMeta::new_readonly(authority_address, true),
            AccountMeta::new(payer_address, true),
            AccountMeta::new_readonly(system_program::id(), false),
        ],
    );

    (instruction, lookup_table_address)
}

/// Constructs an instruction that freezes an address lookup
/// table so that it can never be closed or extended again. Empty
/// lookup tables cannot be frozen.
pub fn freeze_lookup_table(lookup_table_address: Pubkey, authority_address: Pubkey) -> Instruction {
    Instruction::new_with_bincode(
        id(),
        &ProgramInstruction::FreezeLookupTable,
        vec![
            AccountMeta::new(lookup_table_address, false),
            AccountMeta::new_readonly(authority_address, true),
        ],
    )
}

/// Constructs an instruction which extends an address lookup
/// table account with new addresses.
pub fn extend_lookup_table(
    lookup_table_address: Pubkey,
    authority_address: Pubkey,
    payer_address: Pubkey,
    new_addresses: Vec<Pubkey>,
) -> Instruction {
    Instruction::new_with_bincode(
        id(),
        &ProgramInstruction::ExtendLookupTable { new_addresses },
        vec![
            AccountMeta::new(lookup_table_address, false),
            AccountMeta::new_readonly(authority_address, true),
            AccountMeta::new(payer_address, true),
            AccountMeta::new_readonly(system_program::id(), false),
        ],
    )
}

/// Returns an instruction that closes an address lookup table
/// account. The account will be deallocated and the lamports
/// will be drained to the recipient address.
pub fn close_lookup_table(
    lookup_table_address: Pubkey,
    authority_address: Pubkey,
    recipient_address: Pubkey,
) -> Instruction {
    Instruction::new_with_bincode(
        id(),
        &ProgramInstruction::CloseLookupTable,
        vec![
            AccountMeta::new(lookup_table_address, false),
            AccountMeta::new_readonly(authority_address, true),
            AccountMeta::new(recipient_address, false),
        ],
    )
}
