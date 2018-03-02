extern crate silk;

fn main() {
    use silk::accountant_stub::AccountantStub;
    use std::time::Instant;
    use std::net::UdpSocket;
    use silk::event::{generate_keypair, get_pubkey};

    let addr = "127.0.0.1:8000";
    let send_addr = "127.0.0.1:8001";
    let socket = UdpSocket::bind(send_addr).unwrap();
    let mut acc = AccountantStub::new(addr, socket);
    let alice_keypair = generate_keypair();
    let alice_pubkey = get_pubkey(&alice_keypair);
    let txs = 2_000;
    println!("Depositing {} units in Alice's account...", txs);
    let sig = acc.deposit(txs, &alice_keypair).unwrap();
    acc.wait_on_signature(&sig).unwrap();
    assert_eq!(acc.get_balance(&alice_pubkey).unwrap(), txs);
    println!("Done.");

    println!("Transferring 1 unit {} times...", txs);
    let now = Instant::now();
    let mut sig = sig;
    for _ in 0..txs {
        let bob_keypair = generate_keypair();
        let bob_pubkey = get_pubkey(&bob_keypair);
        sig = acc.transfer(1, &alice_keypair, bob_pubkey).unwrap();
    }
    println!("Waiting for last transaction to be confirmed...",);
    acc.wait_on_signature(&sig).unwrap();

    let duration = now.elapsed();
    let ns = duration.as_secs() * 1_000_000_000 + duration.subsec_nanos() as u64;
    let tps = (txs * 1_000_000_000) as f64 / ns as f64;
    println!("Done. {} tps!", tps);
    let val = acc.get_balance(&alice_pubkey).unwrap();
    println!("Alice's Final Balance {}", val);
    assert_eq!(val, 0);
}
