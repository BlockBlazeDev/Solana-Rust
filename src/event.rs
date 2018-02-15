//! The `event` crate provides the foundational data structures for Proof-of-History

/// A Proof-of-History is an ordered log of events in time. Each entry contains three
/// pieces of data. The 'n' field is the number of hashes performed since the previous
/// entry.  The 'hash' field is the result of hashing 'hash' from the previous entry 'n'
/// times.  The 'data' field is an optional foreign key (a hash) pointing to some arbitrary
/// data that a client is looking to associate with the entry.
///
/// If you divide 'n' by the amount of time it takes to generate a new hash, you
/// get a duration estimate since the last event. Since processing power increases
/// over time, one should expect the duration 'n' represents to decrease proportionally.
/// Though processing power varies across nodes, the network gives priority to the
/// fastest processor. Duration should therefore be estimated by assuming that the hash
/// was generated by the fastest processor at the time the entry was logged.
///
/// When 'data' is None, the event represents a simple "tick", and exists for the
/// sole purpose of improving the performance of event log verification. A tick can
/// be generated in 'n' hashes and verified in 'n' hashes.  By logging a hash alongside
/// the tick, each tick and be verified in parallel using the 'hash' of the preceding
/// tick to seed its hashing.
pub struct Event {
    pub hash: u64,
    pub n: u64,
    pub data: Option<u64>,
}

impl Event {
    /// Creates an Event from the number of hashes 'n' since the previous event
    /// and that resulting 'hash'.
    pub fn new(hash: u64, n: u64) -> Self {
        let data = None;
        Event { hash, n, data }
    }

    /// Creates an Event from by hashing 'seed' 'n' times.
    ///
    /// ```
    /// use loomination::event::Event;
    /// assert_eq!(Event::run(0, 1).n, 1)
    /// ```
    pub fn run(seed: u64, n: u64) -> Self {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hash = seed;
        let mut hasher = DefaultHasher::new();
        for _ in 0..n {
            hash.hash(&mut hasher);
            hash = hasher.finish();
        }
        Self::new(hash, n)
    }
    /// Verifies self.hash is the result of hashing a 'seed' 'self.n' times.
    ///
    /// ```
    /// use loomination::event::Event;
    /// assert!(Event::run(0, 0).verify(0)); // base case
    /// assert!(!Event::run(0, 0).verify(1)); // base case, bad
    /// assert!(Event::run(0, 1).verify(0)); // inductive case
    /// assert!(!Event::run(0, 1).verify(1)); // inductive case, bad
    /// ```
    pub fn verify(self: &Self, seed: u64) -> bool {
        self.hash == Self::run(seed, self.n).hash
    }
}

/// Verifies the hashes and counts of a slice of events are all consistent.
///
/// ```
/// use loomination::event::{verify_slice, Event};
/// assert!(verify_slice(&vec![], 0)); // base case
/// assert!(verify_slice(&vec![Event::run(0, 0)], 0)); // singleton case 1
/// assert!(!verify_slice(&vec![Event::run(0, 0)], 1)); // singleton case 2, bad
/// assert!(verify_slice(&vec![Event::run(0, 0), Event::run(0, 0)], 0)); // lazy inductive case
/// assert!(!verify_slice(&vec![Event::run(0, 0), Event::run(1, 0)], 0)); // lazy inductive case, bad
/// ```
pub fn verify_slice(events: &[Event], seed: u64) -> bool {
    use rayon::prelude::*;
    let genesis = [Event::run(seed, 0)];
    let event_pairs = genesis.par_iter().chain(events).zip(events);
    event_pairs.all(|(x, x1)| x1.verify(x.hash))
}

/// Verifies the hashes and events serially. Exists only for reference.
pub fn verify_slice_seq(events: &[Event], seed: u64) -> bool {
    let genesis = [Event::run(seed, 0)];
    let event_pairs = genesis.iter().chain(events).zip(events);
    event_pairs.into_iter().all(|(x, x1)| x1.verify(x.hash))
}

/// Create a vector of Ticks of length 'len' from 'seed' hash and 'hashes_since_prev'.
pub fn create_events(seed: u64, hashes_since_prev: u64, len: usize) -> Vec<Event> {
    use itertools::unfold;
    let mut events = unfold(seed, |state| {
        let event = Event::run(*state, hashes_since_prev);
        *state = event.hash;
        return Some(event);
    });
    events.by_ref().take(len).collect()
}

#[cfg(all(feature = "unstable", test))]
mod bench {
    extern crate test;
    use self::test::Bencher;
    use event;

    #[bench]
    fn event_bench(bencher: &mut Bencher) {
        let seed = 0;
        let events = event::create_events(seed, 100_000, 4);
        bencher.iter(|| {
            assert!(event::verify_slice(&events, seed));
        });
    }
}
