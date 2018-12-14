//! The `solana` library implements the Solana high-performance blockchain architecture.
//! It includes a full Rust implementation of the architecture (see
//! [Fullnode](server/struct.Fullnode.html)) as well as hooks to GPU implementations of its most
//! paralellizable components (i.e. [SigVerify](sigverify/index.html)).  It also includes
//! command-line tools to spin up fullnodes and a Rust library
//! (see [ThinClient](thin_client/struct.ThinClient.html)) to interact with them.
//!

#![cfg_attr(feature = "unstable", feature(test))]
#[macro_use]
pub mod counter;
pub mod bank;
pub mod banking_stage;
pub mod blob_fetch_stage;
pub mod bloom;
pub mod broadcast_service;
#[cfg(feature = "chacha")]
pub mod chacha;
#[cfg(all(feature = "chacha", feature = "cuda"))]
pub mod chacha_cuda;
pub mod client;
pub mod crds;
pub mod crds_gossip;
pub mod crds_gossip_error;
pub mod crds_gossip_pull;
pub mod crds_gossip_push;
pub mod crds_traits_impls;
pub mod crds_value;
pub mod create_vote_account;
#[macro_use]
pub mod contact_info;
pub mod cluster_info;
pub mod compute_leader_finality_service;
pub mod db_ledger;
pub mod db_window;
pub mod entry;
#[cfg(feature = "erasure")]
pub mod erasure;
pub mod fetch_stage;
pub mod fullnode;
pub mod gossip_service;
pub mod leader_scheduler;
pub mod ledger;
pub mod ledger_write_stage;
pub mod mint;
pub mod netutil;
pub mod packet;
pub mod poh;
pub mod poh_recorder;
pub mod poh_service;
pub mod recvmmsg;
pub mod replay_stage;
pub mod replicator;
pub mod result;
pub mod retransmit_stage;
pub mod rpc;
pub mod rpc_pubsub;
pub mod rpc_request;
pub mod runtime;
pub mod service;
pub mod signature;
pub mod sigverify;
pub mod sigverify_stage;
pub mod storage_stage;
pub mod store_ledger_stage;
pub mod streamer;
pub mod test_tx;
pub mod thin_client;
pub mod tpu;
pub mod tpu_forwarder;
pub mod tvu;
pub mod vote_stage;
pub mod window;
pub mod window_service;

#[cfg(test)]
#[cfg(any(feature = "chacha", feature = "cuda"))]
#[macro_use]
extern crate hex_literal;

#[macro_use]
extern crate log;

#[macro_use]
extern crate serde_derive;
#[macro_use]
extern crate serde_json;

use solana_jsonrpc_core as jsonrpc_core;
use solana_jsonrpc_http_server as jsonrpc_http_server;
#[macro_use]
extern crate solana_jsonrpc_macros as jsonrpc_macros;
use solana_jsonrpc_pubsub as jsonrpc_pubsub;
use solana_jsonrpc_ws_server as jsonrpc_ws_server;

#[cfg(test)]
#[macro_use]
extern crate matches;
