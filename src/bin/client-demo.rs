extern crate silk;

fn main() {
    use silk::accountant_stub::AccountantStub;
    use std::thread::sleep;
    use std::time::Duration;
    use silk::log::{generate_keypair, get_pubkey};

    let addr = "127.0.0.1:8000";
    let mut acc = AccountantStub::new(addr);
    let alice_keypair = generate_keypair();
    let bob_keypair = generate_keypair();
    println!("Depositing...");
    acc.deposit(10_000, &alice_keypair).unwrap();
    acc.deposit(1_000, &bob_keypair).unwrap();
    println!("Done.");

    sleep(Duration::from_millis(30));
    let bob_pubkey = get_pubkey(&bob_keypair);
    println!("Transferring...");
    acc.transfer(500, &alice_keypair, bob_pubkey).unwrap();
    println!("Done.");

    sleep(Duration::from_millis(30));
    println!("Done. Checking balance.");
    println!("Balance {}", acc.get_balance(&bob_pubkey).unwrap());
}
