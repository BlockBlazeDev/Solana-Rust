#!/bin/bash
cd /home/ubuntu/solana
git pull
export RUST_LOG=solana::crdt=trace
cat genesis.log | cargo run --release --bin --features cuda solana-testnode -- -s leader.json -b 8000 -d
