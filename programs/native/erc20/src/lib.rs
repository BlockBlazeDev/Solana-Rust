//! The `erc20` library implements a generic erc20-like token

extern crate bincode;
extern crate env_logger;
#[macro_use]
extern crate log;
extern crate serde;
#[macro_use]
extern crate serde_derive;
#[macro_use]
extern crate solana_sdk;

use solana_sdk::account::KeyedAccount;
use std::sync::{Once, ONCE_INIT};

mod token_program;

solana_entrypoint!(entrypoint);
fn entrypoint(info: &mut [KeyedAccount], input: &[u8], _tick_height: u64) -> bool {
    // env_logger can only be initialized once
    static INIT: Once = ONCE_INIT;
    INIT.call_once(env_logger::init);

    match token_program::TokenProgram::process(info, input) {
        Err(err) => {
            error!("error: {:?}", err);
            false
        }
        Ok(_) => true,
    }
}
