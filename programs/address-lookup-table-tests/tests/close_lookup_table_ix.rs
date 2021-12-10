use {
    assert_matches::assert_matches,
    common::{
        add_lookup_table_account, assert_ix_error, new_address_lookup_table,
        overwrite_slot_hashes_with_slots, setup_test_context,
    },
    solana_address_lookup_table_program::instruction::close_lookup_table,
    solana_program_test::*,
    solana_sdk::{
        instruction::InstructionError,
        pubkey::Pubkey,
        signature::{Keypair, Signer},
        transaction::Transaction,
    },
};

mod common;

#[tokio::test]
async fn test_close_lookup_table() {
    let mut context = setup_test_context().await;
    overwrite_slot_hashes_with_slots(&mut context, &[]);

    let authority_keypair = Keypair::new();
    let initialized_table = new_address_lookup_table(Some(authority_keypair.pubkey()), 0);
    let lookup_table_address = Pubkey::new_unique();
    add_lookup_table_account(&mut context, lookup_table_address, initialized_table).await;

    let client = &mut context.banks_client;
    let payer = &context.payer;
    let recent_blockhash = context.last_blockhash;
    let transaction = Transaction::new_signed_with_payer(
        &[close_lookup_table(
            lookup_table_address,
            authority_keypair.pubkey(),
            context.payer.pubkey(),
        )],
        Some(&payer.pubkey()),
        &[payer, &authority_keypair],
        recent_blockhash,
    );

    assert_matches!(client.process_transaction(transaction).await, Ok(()));
    assert!(client
        .get_account(lookup_table_address)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn test_close_lookup_table_too_recent() {
    let mut context = setup_test_context().await;

    let authority_keypair = Keypair::new();
    let initialized_table = new_address_lookup_table(Some(authority_keypair.pubkey()), 0);
    let lookup_table_address = Pubkey::new_unique();
    add_lookup_table_account(&mut context, lookup_table_address, initialized_table).await;

    let ix = close_lookup_table(
        lookup_table_address,
        authority_keypair.pubkey(),
        context.payer.pubkey(),
    );

    // Context sets up the slot hashes sysvar to have an entry
    // for slot 0 which is what the default initialized table
    // has as its derivation slot. Because that slot is present,
    // the ix should fail.
    assert_ix_error(
        &mut context,
        ix,
        Some(&authority_keypair),
        InstructionError::InvalidArgument,
    )
    .await;
}

#[tokio::test]
async fn test_close_immutable_lookup_table() {
    let mut context = setup_test_context().await;

    let initialized_table = new_address_lookup_table(None, 10);
    let lookup_table_address = Pubkey::new_unique();
    add_lookup_table_account(&mut context, lookup_table_address, initialized_table).await;

    let authority = Keypair::new();
    let ix = close_lookup_table(
        lookup_table_address,
        authority.pubkey(),
        Pubkey::new_unique(),
    );

    assert_ix_error(
        &mut context,
        ix,
        Some(&authority),
        InstructionError::Immutable,
    )
    .await;
}

#[tokio::test]
async fn test_close_lookup_table_with_wrong_authority() {
    let mut context = setup_test_context().await;

    let authority = Keypair::new();
    let wrong_authority = Keypair::new();
    let initialized_table = new_address_lookup_table(Some(authority.pubkey()), 10);
    let lookup_table_address = Pubkey::new_unique();
    add_lookup_table_account(&mut context, lookup_table_address, initialized_table).await;

    let ix = close_lookup_table(
        lookup_table_address,
        wrong_authority.pubkey(),
        Pubkey::new_unique(),
    );

    assert_ix_error(
        &mut context,
        ix,
        Some(&wrong_authority),
        InstructionError::IncorrectAuthority,
    )
    .await;
}

#[tokio::test]
async fn test_close_lookup_table_without_signing() {
    let mut context = setup_test_context().await;

    let authority = Keypair::new();
    let initialized_table = new_address_lookup_table(Some(authority.pubkey()), 10);
    let lookup_table_address = Pubkey::new_unique();
    add_lookup_table_account(&mut context, lookup_table_address, initialized_table).await;

    let mut ix = close_lookup_table(
        lookup_table_address,
        authority.pubkey(),
        Pubkey::new_unique(),
    );
    ix.accounts[1].is_signer = false;

    assert_ix_error(
        &mut context,
        ix,
        None,
        InstructionError::MissingRequiredSignature,
    )
    .await;
}
