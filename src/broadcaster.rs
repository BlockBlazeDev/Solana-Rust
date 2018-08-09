//! The `broadcaster` broadcasts data from a leader node to validators
//!
use counter::Counter;
use crdt::{Crdt, CrdtError, NodeInfo};
#[cfg(feature = "erasure")]
use erasure;
use log::Level;
use packet::BlobRecycler;
use result::{Error, Result};
use std::mem;
use std::net::UdpSocket;
use std::sync::atomic::AtomicUsize;
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, RwLock};
use std::thread::{Builder, JoinHandle};
use std::time::Duration;
use streamer::BlobReceiver;
use window::{self, SharedWindow, WindowIndex, WINDOW_SIZE};

fn broadcast(
    node_info: &NodeInfo,
    broadcast_table: &[NodeInfo],
    window: &SharedWindow,
    recycler: &BlobRecycler,
    r: &BlobReceiver,
    sock: &UdpSocket,
    transmit_index: &mut WindowIndex,
    receive_index: &mut u64,
) -> Result<()> {
    let debug_id = node_info.debug_id();
    let timer = Duration::new(1, 0);
    let mut dq = r.recv_timeout(timer)?;
    while let Ok(mut nq) = r.try_recv() {
        dq.append(&mut nq);
    }

    // flatten deque to vec
    let blobs_vec: Vec<_> = dq.into_iter().collect();

    // We could receive more blobs than window slots so
    // break them up into window-sized chunks to process
    let blobs_chunked = blobs_vec.chunks(WINDOW_SIZE as usize).map(|x| x.to_vec());

    if log_enabled!(Level::Trace) {
        trace!("{}", window::print_window(debug_id, window, *receive_index));
    }

    for mut blobs in blobs_chunked {
        let blobs_len = blobs.len();
        trace!("{:x}: broadcast blobs.len: {}", debug_id, blobs_len);

        // Index the blobs
        window::index_blobs(node_info, &blobs, receive_index)
            .expect("index blobs for initial window");

        // keep the cache of blobs that are broadcast
        inc_new_counter_info!("streamer-broadcast-sent", blobs.len());
        {
            let mut win = window.write().unwrap();
            assert!(blobs.len() <= win.len());
            for b in &blobs {
                let ix = b.read().unwrap().get_index().expect("blob index");
                let pos = (ix % WINDOW_SIZE) as usize;
                if let Some(x) = mem::replace(&mut win[pos].data, None) {
                    trace!(
                        "{:x} popped {} at {}",
                        debug_id,
                        x.read().unwrap().get_index().unwrap(),
                        pos
                    );
                    recycler.recycle(x);
                }
                if let Some(x) = mem::replace(&mut win[pos].coding, None) {
                    trace!(
                        "{:x} popped {} at {}",
                        debug_id,
                        x.read().unwrap().get_index().unwrap(),
                        pos
                    );
                    recycler.recycle(x);
                }

                trace!("{:x} null {}", debug_id, pos);
            }
            while let Some(b) = blobs.pop() {
                let ix = b.read().unwrap().get_index().expect("blob index");
                let pos = (ix % WINDOW_SIZE) as usize;
                trace!("{:x} caching {} at {}", debug_id, ix, pos);
                assert!(win[pos].data.is_none());
                win[pos].data = Some(b);
            }
        }

        // Fill in the coding blob data from the window data blobs
        #[cfg(feature = "erasure")]
        {
            erasure::generate_coding(
                debug_id,
                &mut window.write().unwrap(),
                recycler,
                *receive_index,
                blobs_len,
                &mut transmit_index.coding,
            )?;
        }

        *receive_index += blobs_len as u64;

        // Send blobs out from the window
        Crdt::broadcast(
            &node_info,
            &broadcast_table,
            &window,
            &sock,
            transmit_index,
            *receive_index,
        )?;
    }
    Ok(())
}

/// Service to broadcast messages from the leader to layer 1 nodes.
/// See `crdt` for network layer definitions.
/// # Arguments
/// * `sock` - Socket to send from.
/// * `exit` - Boolean to signal system exit.
/// * `crdt` - CRDT structure
/// * `window` - Cache of blobs that we have broadcast
/// * `recycler` - Blob recycler.
/// * `r` - Receive channel for blobs to be retransmitted to all the layer 1 nodes.
pub fn broadcaster(
    sock: UdpSocket,
    crdt: Arc<RwLock<Crdt>>,
    window: SharedWindow,
    entry_height: u64,
    recycler: BlobRecycler,
    r: BlobReceiver,
) -> JoinHandle<()> {
    Builder::new()
        .name("solana-broadcaster".to_string())
        .spawn(move || {
            let mut transmit_index = WindowIndex {
                data: entry_height,
                coding: entry_height,
            };
            let mut receive_index = entry_height;
            let me = crdt.read().unwrap().my_data().clone();
            loop {
                let broadcast_table = crdt.read().unwrap().compute_broadcast_table();
                if let Err(e) = broadcast(
                    &me,
                    &broadcast_table,
                    &window,
                    &recycler,
                    &r,
                    &sock,
                    &mut transmit_index,
                    &mut receive_index,
                ) {
                    match e {
                        Error::RecvTimeoutError(RecvTimeoutError::Disconnected) => break,
                        Error::RecvTimeoutError(RecvTimeoutError::Timeout) => (),
                        Error::CrdtError(CrdtError::NoPeers) => (), // TODO: Why are the unit-tests throwing hundreds of these?
                        _ => {
                            inc_new_counter_info!("streamer-broadcaster-error", 1, 1);
                            error!("broadcaster error: {:?}", e);
                        }
                    }
                }
            }
        })
        .unwrap()
}

fn retransmit(
    crdt: &Arc<RwLock<Crdt>>,
    recycler: &BlobRecycler,
    r: &BlobReceiver,
    sock: &UdpSocket,
) -> Result<()> {
    let timer = Duration::new(1, 0);
    let mut dq = r.recv_timeout(timer)?;
    while let Ok(mut nq) = r.try_recv() {
        dq.append(&mut nq);
    }
    {
        for b in &dq {
            Crdt::retransmit(&crdt, b, sock)?;
        }
    }
    while let Some(b) = dq.pop_front() {
        recycler.recycle(b);
    }
    Ok(())
}

/// Service to retransmit messages from the leader to layer 1 nodes.
/// See `crdt` for network layer definitions.
/// # Arguments
/// * `sock` - Socket to read from.  Read timeout is set to 1.
/// * `exit` - Boolean to signal system exit.
/// * `crdt` - This structure needs to be updated and populated by the bank and via gossip.
/// * `recycler` - Blob recycler.
/// * `r` - Receive channel for blobs to be retransmitted to all the layer 1 nodes.
pub fn retransmitter(
    sock: UdpSocket,
    crdt: Arc<RwLock<Crdt>>,
    recycler: BlobRecycler,
    r: BlobReceiver,
) -> JoinHandle<()> {
    Builder::new()
        .name("solana-retransmitter".to_string())
        .spawn(move || {
            trace!("retransmitter started");
            loop {
                if let Err(e) = retransmit(&crdt, &recycler, &r, &sock) {
                    match e {
                        Error::RecvTimeoutError(RecvTimeoutError::Disconnected) => break,
                        Error::RecvTimeoutError(RecvTimeoutError::Timeout) => (),
                        _ => {
                            inc_new_counter_info!("streamer-retransmit-error", 1, 1);
                            error!("retransmitter error: {:?}", e);
                        }
                    }
                }
            }
            trace!("exiting retransmitter");
        })
        .unwrap()
}
