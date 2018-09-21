//! system program

use bank::Account;
use bincode::deserialize;
use signature::Pubkey;
use transaction::Transaction;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum SystemProgram {
    /// Create a new account
    /// * Transaction::keys[0] - source
    /// * Transaction::keys[1] - new account key
    /// * tokens - number of tokens to transfer to the new account
    /// * space - memory to allocate if greater then zero
    /// * program_id - the program id of the new account
    CreateAccount {
        tokens: i64,
        space: u64,
        program_id: Pubkey,
    },
    /// Assign account to a program
    /// * Transaction::keys[0] - account to assign
    Assign { program_id: Pubkey },
    /// Move tokens
    /// * Transaction::keys[0] - source
    /// * Transaction::keys[1] - destination
    Move { tokens: i64 },
}

pub const SYSTEM_PROGRAM_ID: [u8; 32] = [0u8; 32];

impl SystemProgram {
    pub fn check_id(program_id: &Pubkey) -> bool {
        program_id.as_ref() == SYSTEM_PROGRAM_ID
    }

    pub fn id() -> Pubkey {
        Pubkey::new(&SYSTEM_PROGRAM_ID)
    }
    pub fn get_balance(account: &Account) -> i64 {
        account.tokens
    }
    pub fn process_transaction(tx: &Transaction, accounts: &mut [Account]) {
        if let Ok(syscall) = deserialize(&tx.userdata){
            trace!("process_transaction: {:?}", syscall);
            match syscall {
                SystemProgram::CreateAccount {
                    tokens,
                    space,
                    program_id,
                } => {
                    if !Self::check_id(&accounts[0].program_id) {
                        return;
                    }
                    if space > 0
                        && (!accounts[1].userdata.is_empty()
                            || !Self::check_id(&accounts[1].program_id))
                    {
                        return;
                    }
                    accounts[0].tokens -= tokens;
                    accounts[1].tokens += tokens;
                    accounts[1].program_id = program_id;
                    accounts[1].userdata = vec![0; space as usize];
                }
                SystemProgram::Assign { program_id } => {
                    if !Self::check_id(&accounts[0].program_id) {
                        return;
                    }
                    accounts[0].program_id = program_id;
                }
                SystemProgram::Move { tokens } => {
                    //bank should be verifying correctness
                    accounts[0].tokens -= tokens;
                    accounts[1].tokens += tokens;
                }
            }
        } else {
            info!("Invalid transaction userdata: {:?}", tx.userdata);
        }
    }
}
#[cfg(test)]
mod test {
    use bank::Account;
    use hash::Hash;
    use signature::{Keypair, KeypairUtil, Pubkey};
    use system_program::SystemProgram;
    use transaction::Transaction;
    #[test]
    fn test_create_noop() {
        let from = Keypair::new();
        let to = Keypair::new();
        let mut accounts = vec![Account::default(), Account::default()];
        let tx = Transaction::system_new(&from, to.pubkey(), 0, Hash::default());
        SystemProgram::process_transaction(&tx, &mut accounts);
        assert_eq!(accounts[0].tokens, 0);
        assert_eq!(accounts[1].tokens, 0);
    }
    #[test]
    fn test_create_spend() {
        let from = Keypair::new();
        let to = Keypair::new();
        let mut accounts = vec![Account::default(), Account::default()];
        accounts[0].tokens = 1;
        let tx = Transaction::system_new(&from, to.pubkey(), 1, Hash::default());
        SystemProgram::process_transaction(&tx, &mut accounts);
        assert_eq!(accounts[0].tokens, 0);
        assert_eq!(accounts[1].tokens, 1);
    }
    #[test]
    fn test_create_spend_wrong_source() {
        let from = Keypair::new();
        let to = Keypair::new();
        let mut accounts = vec![Account::default(), Account::default()];
        accounts[0].tokens = 1;
        accounts[0].program_id = from.pubkey();
        let tx = Transaction::system_new(&from, to.pubkey(), 1, Hash::default());
        SystemProgram::process_transaction(&tx, &mut accounts);
        assert_eq!(accounts[0].tokens, 1);
        assert_eq!(accounts[1].tokens, 0);
    }
    #[test]
    fn test_create_assign_and_allocate() {
        let from = Keypair::new();
        let to = Keypair::new();
        let mut accounts = vec![Account::default(), Account::default()];
        let tx =
            Transaction::system_create(&from, to.pubkey(), Hash::default(), 0, 1, to.pubkey(), 0);
        SystemProgram::process_transaction(&tx, &mut accounts);
        assert!(accounts[0].userdata.is_empty());
        assert_eq!(accounts[1].userdata.len(), 1);
        assert_eq!(accounts[1].program_id, to.pubkey());
    }
    #[test]
    fn test_create_allocate_wrong_dest_program() {
        let from = Keypair::new();
        let to = Keypair::new();
        let mut accounts = vec![Account::default(), Account::default()];
        accounts[1].program_id = to.pubkey();
        let tx = Transaction::system_create(
            &from,
            to.pubkey(),
            Hash::default(),
            0,
            1,
            Pubkey::default(),
            0,
        );
        SystemProgram::process_transaction(&tx, &mut accounts);
        assert!(accounts[1].userdata.is_empty());
    }
    #[test]
    fn test_create_allocate_wrong_source_program() {
        let from = Keypair::new();
        let to = Keypair::new();
        let mut accounts = vec![Account::default(), Account::default()];
        accounts[0].program_id = to.pubkey();
        let tx = Transaction::system_create(
            &from,
            to.pubkey(),
            Hash::default(),
            0,
            1,
            Pubkey::default(),
            0,
        );
        SystemProgram::process_transaction(&tx, &mut accounts);
        assert!(accounts[1].userdata.is_empty());
    }
    #[test]
    fn test_create_allocate_already_allocated() {
        let from = Keypair::new();
        let to = Keypair::new();
        let mut accounts = vec![Account::default(), Account::default()];
        accounts[1].userdata = vec![0, 0, 0];
        let tx = Transaction::system_create(
            &from,
            to.pubkey(),
            Hash::default(),
            0,
            2,
            Pubkey::default(),
            0,
        );
        SystemProgram::process_transaction(&tx, &mut accounts);
        assert_eq!(accounts[1].userdata.len(), 3);
    }
    #[test]
    fn test_create_assign() {
        let from = Keypair::new();
        let program = Keypair::new();
        let mut accounts = vec![Account::default()];
        let tx = Transaction::system_assign(&from, Hash::default(), program.pubkey(), 0);
        SystemProgram::process_transaction(&tx, &mut accounts);
        assert_eq!(accounts[0].program_id, program.pubkey());
    }
    #[test]
    fn test_move() {
        let from = Keypair::new();
        let to = Keypair::new();
        let mut accounts = vec![Account::default(), Account::default()];
        accounts[0].tokens = 1;
        let tx = Transaction::new(&from, to.pubkey(), 1, Hash::default());
        SystemProgram::process_transaction(&tx, &mut accounts);
        assert_eq!(accounts[0].tokens, 0);
        assert_eq!(accounts[1].tokens, 1);
    }

    /// Detect binary changes in the serialized program userdata, which could have a downstream
    /// affect on SDKs and DApps
    #[test]
    fn test_sdk_serialize() {
        let keypair = Keypair::new();
        use budget_program::BUDGET_PROGRAM_ID;

        // CreateAccount
        let tx = Transaction::system_create(
            &keypair,
            keypair.pubkey(),
            Hash::default(),
            111,
            222,
            Pubkey::new(&BUDGET_PROGRAM_ID),
            0,
        );

        assert_eq!(
            tx.userdata,
            vec![
                0, 0, 0, 0, 111, 0, 0, 0, 0, 0, 0, 0, 222, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
            ]
        );

        // CreateAccount
        let tx = Transaction::system_create(
            &keypair,
            keypair.pubkey(),
            Hash::default(),
            111,
            222,
            Pubkey::default(),
            0,
        );

        assert_eq!(
            tx.userdata,
            vec![
                0, 0, 0, 0, 111, 0, 0, 0, 0, 0, 0, 0, 222, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
            ]
        );

        // Assign
        let tx = Transaction::system_assign(
            &keypair,
            Hash::default(),
            Pubkey::new(&BUDGET_PROGRAM_ID),
            0,
        );
        assert_eq!(
            tx.userdata,
            vec![
                1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0
            ]
        );

        // Move
        let tx = Transaction::system_move(&keypair, keypair.pubkey(), 123, Hash::default(), 0);
        assert_eq!(tx.userdata, vec![2, 0, 0, 0, 123, 0, 0, 0, 0, 0, 0, 0]);
    }
}
