extern crate silk;

use silk::historian::Historian;
use silk::log::{verify_slice, Entry, Event, Sha256Hash};
use std::thread::sleep;
use std::time::Duration;
use std::sync::mpsc::SendError;

fn create_log(hist: &Historian<Sha256Hash>) -> Result<(), SendError<Event<Sha256Hash>>> {
    sleep(Duration::from_millis(15));
    let data = Sha256Hash::default();
    hist.sender.send(Event::Discovery { data })?;
    sleep(Duration::from_millis(10));
    Ok(())
}

fn main() {
    let seed = Sha256Hash::default();
    let hist = Historian::new(&seed, Some(10));
    create_log(&hist).expect("send error");
    drop(hist.sender);
    let entries: Vec<Entry<Sha256Hash>> = hist.receiver.iter().collect();
    for entry in &entries {
        println!("{:?}", entry);
    }
    assert!(verify_slice(&entries, &seed));
}
