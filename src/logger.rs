//! The `logger` crate provides an object for generating a Proof-of-History.
//! It logs Event items on behalf of its users. It continuously generates
//! new hashes, only stopping to check if it has been sent an Event item. It
//! tags each Event with an Entry and sends it back. The Entry includes the
//! Event, the latest hash, and the number of hashes since the last event.
//! The resulting stream of entries represents ordered events in time.

use std::collections::HashSet;
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError};
use std::time::{Duration, Instant};
use log::{create_entry_mut, Entry, Sha256Hash};
use event::{get_signature, verify_event, Event, Signature};
use serde::Serialize;
use std::fmt::Debug;

#[derive(Debug, PartialEq, Eq)]
pub enum ExitReason {
    RecvDisconnected,
    SendDisconnected,
}

pub struct Logger<T> {
    pub sender: SyncSender<Entry<T>>,
    pub receiver: Receiver<Event<T>>,
    pub last_id: Sha256Hash,
    pub num_hashes: u64,
    pub num_ticks: u64,
}

pub fn verify_event_and_reserve_signature<T: Serialize>(
    signatures: &mut HashSet<Signature>,
    event: &Event<T>,
) -> bool {
    if !verify_event(&event) {
        return false;
    }
    if let Some(sig) = get_signature(&event) {
        if signatures.contains(&sig) {
            return false;
        }
        signatures.insert(sig);
    }
    true
}

impl<T: Serialize + Clone + Debug> Logger<T> {
    pub fn new(
        receiver: Receiver<Event<T>>,
        sender: SyncSender<Entry<T>>,
        start_hash: Sha256Hash,
    ) -> Self {
        Logger {
            receiver,
            sender,
            last_id: start_hash,
            num_hashes: 0,
            num_ticks: 0,
        }
    }

    pub fn log_event(&mut self, event: Event<T>) -> Result<(), (Entry<T>, ExitReason)> {
        let entry = create_entry_mut(&mut self.last_id, &mut self.num_hashes, event);
        if let Err(_) = self.sender.send(entry.clone()) {
            return Err((entry, ExitReason::SendDisconnected));
        }
        Ok(())
    }

    pub fn log_events(
        &mut self,
        epoch: Instant,
        ms_per_tick: Option<u64>,
    ) -> Result<(), (Entry<T>, ExitReason)> {
        loop {
            if let Some(ms) = ms_per_tick {
                if epoch.elapsed() > Duration::from_millis((self.num_ticks + 1) * ms) {
                    self.log_event(Event::Tick)?;
                    self.num_ticks += 1;
                }
            }
            match self.receiver.try_recv() {
                Ok(event) => {
                    self.log_event(event)?;
                }
                Err(TryRecvError::Empty) => {
                    return Ok(());
                }
                Err(TryRecvError::Disconnected) => {
                    let entry = Entry {
                        id: self.last_id,
                        num_hashes: self.num_hashes,
                        event: Event::Tick,
                    };
                    return Err((entry, ExitReason::RecvDisconnected));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::*;
    use event::*;
    use genesis::*;
    use std::sync::mpsc::sync_channel;

    #[test]
    fn test_bad_event_signature() {
        let keypair = generate_keypair();
        let sig = sign_claim_data(&hash(b"hello, world"), &keypair);
        let event0 = Event::new_claim(get_pubkey(&keypair), hash(b"goodbye cruel world"), sig);
        let mut sigs = HashSet::new();
        assert!(!verify_event_and_reserve_signature(&mut sigs, &event0));
        assert!(!sigs.contains(&sig));
    }

    #[test]
    fn test_duplicate_event_signature() {
        let keypair = generate_keypair();
        let to = get_pubkey(&keypair);
        let data = &hash(b"hello, world");
        let sig = sign_claim_data(data, &keypair);
        let event0 = Event::new_claim(to, data, sig);
        let mut sigs = HashSet::new();
        assert!(verify_event_and_reserve_signature(&mut sigs, &event0));
        assert!(!verify_event_and_reserve_signature(&mut sigs, &event0));
    }

    fn run_genesis(gen: Genesis) -> Vec<Entry<u64>> {
        let (_sender, event_receiver) = sync_channel(100);
        let (entry_sender, receiver) = sync_channel(100);
        let mut logger = Logger::new(event_receiver, entry_sender, hash(&gen.pkcs8));
        for tx in gen.create_events() {
            logger.log_event(tx).unwrap();
        }
        drop(logger.sender);
        receiver.iter().collect::<Vec<_>>()
    }

    #[test]
    fn test_genesis_no_creators() {
        let entries = run_genesis(Genesis::new(100, vec![]));
        assert!(verify_slice_u64(&entries, &entries[0].id));
    }

    #[test]
    fn test_genesis() {
        let entries = run_genesis(Genesis::new(100, vec![Creator::new(42)]));
        assert!(verify_slice_u64(&entries, &entries[0].id));
    }
}
