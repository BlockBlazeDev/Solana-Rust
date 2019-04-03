use crate::bank::Bank;
use solana_sdk::instruction::Instruction;
use solana_sdk::message::Message;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;
use solana_sdk::signature::{Keypair, KeypairUtil};
use solana_sdk::sync_client::SyncClient;
use solana_sdk::system_instruction;
use solana_sdk::transaction::{Transaction, TransactionError};

pub struct BankClient<'a> {
    bank: &'a Bank,
}

impl<'a> SyncClient for BankClient<'a> {
    fn send_message(
        &self,
        keypairs: &[&Keypair],
        message: Message,
    ) -> Result<Signature, TransactionError> {
        let blockhash = self.bank.last_blockhash();
        let transaction = Transaction::new(&keypairs, message, blockhash);
        self.bank.process_transaction(&transaction)?;
        Ok(transaction.signatures.get(0).cloned().unwrap_or_default())
    }

    /// Create and process a transaction from a single instruction.
    fn send_instruction(
        &self,
        keypair: &Keypair,
        instruction: Instruction,
    ) -> Result<Signature, TransactionError> {
        let message = Message::new(vec![instruction]);
        self.send_message(&[keypair], message)
    }

    /// Transfer `lamports` from `keypair` to `pubkey`
    fn transfer(
        &self,
        lamports: u64,
        keypair: &Keypair,
        pubkey: &Pubkey,
    ) -> Result<Signature, TransactionError> {
        let move_instruction = system_instruction::transfer(&keypair.pubkey(), pubkey, lamports);
        self.send_instruction(keypair, move_instruction)
    }

    fn get_account_data(&self, pubkey: &Pubkey) -> Option<Vec<u8>> {
        let account = self.bank.get_account(pubkey)?;
        Some(account.data)
    }

    fn get_balance(&self, pubkey: &Pubkey) -> u64 {
        self.bank.get_balance(pubkey)
    }
}

impl<'a> BankClient<'a> {
    pub fn new(bank: &'a Bank) -> Self {
        Self { bank }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::genesis_block::GenesisBlock;
    use solana_sdk::instruction::AccountMeta;

    #[test]
    fn test_bank_client_new_with_keypairs() {
        let (genesis_block, john_doe_keypair) = GenesisBlock::new(10_000);
        let john_pubkey = john_doe_keypair.pubkey();
        let jane_doe_keypair = Keypair::new();
        let jane_pubkey = jane_doe_keypair.pubkey();
        let doe_keypairs = vec![&john_doe_keypair, &jane_doe_keypair];
        let bank = Bank::new(&genesis_block);
        let bank_client = BankClient::new(&bank);

        // Create 2-2 Multisig Transfer instruction.
        let bob_pubkey = Pubkey::new_rand();
        let mut move_instruction = system_instruction::transfer(&john_pubkey, &bob_pubkey, 42);
        move_instruction
            .accounts
            .push(AccountMeta::new(jane_pubkey, true));

        let message = Message::new(vec![move_instruction]);
        bank_client.send_message(&doe_keypairs, message).unwrap();
        assert_eq!(bank_client.get_balance(&bob_pubkey), 42);
    }
}
