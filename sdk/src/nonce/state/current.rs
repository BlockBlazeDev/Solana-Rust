use super::Versions;
use crate::{hash::Hash, pubkey::Pubkey};
use serde_derive::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize, PartialEq, Clone, Copy)]
pub struct Data {
    pub authority: Pubkey,
    pub blockhash: Hash,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone, Copy)]
pub enum State {
    Uninitialized,
    Initialized(Data),
}

impl Default for State {
    fn default() -> Self {
        State::Uninitialized
    }
}

impl State {
    pub fn size() -> usize {
        let data = Versions::new_current(State::Initialized(Data::default()));
        bincode::serialized_size(&data).unwrap() as usize
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn default_is_uninitialized() {
        assert_eq!(State::default(), State::Uninitialized)
    }
}
