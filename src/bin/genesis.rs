//! A command-line executable for generating the chain's genesis block.

extern crate atty;
extern crate serde_json;
extern crate solana;

use atty::{is, Stream};
use solana::mint::Mint;
use std::error;
use std::io::{stdin, stdout, Read, Write};
use std::process::exit;

fn main() -> Result<(), Box<error::Error>> {
    if is(Stream::Stdin) {
        eprintln!("nothing found on stdin, expected a json file");
        exit(1);
    }

    let mut buffer = String::new();
    let num_bytes = stdin().read_to_string(&mut buffer)?;
    if num_bytes == 0 {
        eprintln!("empty file on stdin, expected a json file");
        exit(1);
    }

    let mint: Mint = serde_json::from_str(&buffer)?;
    let mut writer = stdout();
    for x in mint.create_entries() {
        writeln!(writer, "{}", serde_json::to_string(&x)?)?;
    }
    Ok(())
}
