#[cfg(any(feature = "bpf_c", feature = "bpf_rust"))]
mod bpf {
    use solana_runtime::bank::Bank;
    use solana_runtime::loader_utils::load_program;
    use solana_sdk::genesis_block::GenesisBlock;
    use solana_sdk::native_loader;
    use solana_sdk::transaction::Transaction;
    use std::env;
    use std::fs::File;
    use std::path::PathBuf;

    /// BPF program file extension
    const PLATFORM_FILE_EXTENSION_BPF: &str = "so";

    /// Create a BPF program file name
    fn create_bpf_path(name: &str) -> PathBuf {
        let mut pathbuf = {
            let current_exe = env::current_exe().unwrap();
            PathBuf::from(current_exe.parent().unwrap().parent().unwrap())
        };
        pathbuf.push("bpf/");
        pathbuf.push(name);
        pathbuf.set_extension(PLATFORM_FILE_EXTENSION_BPF);
        pathbuf
    }

    #[cfg(feature = "bpf_c")]
    mod bpf_c {
        use super::*;
        use solana_sdk::bpf_loader;
        use std::io::Read;

        #[test]
        fn test_program_bpf_c_noop() {
            solana_logger::setup();

            let mut file = File::open(create_bpf_path("noop")).expect("file open failed");
            let mut elf = Vec::new();
            file.read_to_end(&mut elf).unwrap();

            let (genesis_block, mint_keypair) = GenesisBlock::new(50);
            let bank = Bank::new(&genesis_block);

            // Call user program
            let program_id = load_program(&bank, &mint_keypair, &bpf_loader::id(), elf);
            let tx = Transaction::new(
                &mint_keypair,
                &[],
                &program_id,
                &vec![1u8],
                bank.last_blockhash(),
                0,
            );
            bank.process_transaction(&tx).unwrap();
            assert_eq!(bank.get_signature_status(&tx.signatures[0]), Some(Ok(())));
        }

        #[test]
        fn test_program_bpf_c() {
            solana_logger::setup();

            let programs = [
                "bpf_to_bpf",
                "multiple_static",
                "noop",
                "noop++",
                "relative_call",
                "struct_pass",
                "struct_ret",
            ];
            for program in programs.iter() {
                println!("Test program: {:?}", program);
                let mut file = File::open(create_bpf_path(program)).expect("file open failed");
                let mut elf = Vec::new();
                file.read_to_end(&mut elf).unwrap();

                let (genesis_block, mint_keypair) = GenesisBlock::new(50);
                let bank = Bank::new(&genesis_block);

                let loader_id = load_program(
                    &bank,
                    &mint_keypair,
                    &native_loader::id(),
                    "solana_bpf_loader".as_bytes().to_vec(),
                );

                // Call user program
                let program_id = load_program(&bank, &mint_keypair, &loader_id, elf);
                let tx = Transaction::new(
                    &mint_keypair,
                    &[],
                    &program_id,
                    &vec![1u8],
                    bank.last_blockhash(),
                    0,
                );
                bank.process_transaction(&tx).unwrap();
                assert_eq!(bank.get_signature_status(&tx.signatures[0]), Some(Ok(())));
            }
        }
    }

    // Cannot currently build the Rust BPF program as part
    // of the rest of the build due to recursive `cargo build` causing
    // a build deadlock.  Therefore you must build the Rust programs
    // yourself first by calling `make all` in the Rust BPF program's directory
    #[cfg(feature = "bpf_rust")]
    mod bpf_rust {
        use super::*;
        use std::io::Read;

        #[test]
        fn test_program_bpf_rust() {
            solana_logger::setup();

            let programs = ["solana_bpf_rust_noop"];
            for program in programs.iter() {
                let filename = create_bpf_path(program);
                println!("Test program: {:?} from {:?}", program, filename);
                let mut file = File::open(filename).unwrap();
                let mut elf = Vec::new();
                file.read_to_end(&mut elf).unwrap();

                let (genesis_block, mint_keypair) = GenesisBlock::new(50);
                let bank = Bank::new(&genesis_block);
                let loader_id = load_program(
                    &bank,
                    &mint_keypair,
                    &native_loader::id(),
                    "solana_bpf_loader".as_bytes().to_vec(),
                );

                // Call user program
                let program_id = load_program(&bank, &mint_keypair, &loader_id, elf);
                let tx = Transaction::new(
                    &mint_keypair,
                    &[],
                    &program_id,
                    &vec![1u8],
                    bank.last_blockhash(),
                    0,
                );
                bank.process_transaction(&tx).unwrap();
                assert_eq!(bank.get_signature_status(&tx.signatures[0]), Some(Ok(())));
            }
        }
    }
}
