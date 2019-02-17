//! The `pubsub` module implements a threaded subscription service on client RPC request

use crate::bank::Bank;
use crate::rpc::RpcSignatureStatus;
use crate::rpc_subscriptions::RpcSubscriptions;
use crate::service::Service;
use bs58;
use jsonrpc_core::futures::Future;
use jsonrpc_core::{Error, ErrorCode, Result};
use jsonrpc_derive::rpc;
use jsonrpc_pubsub::typed::Subscriber;
use jsonrpc_pubsub::{PubSubHandler, Session, SubscriptionId};
use jsonrpc_ws_server::{RequestContext, ServerBuilder};
use solana_sdk::account::Account;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;
use std::mem;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{atomic, Arc, RwLock};
use std::thread::{self, sleep, Builder, JoinHandle};
use std::time::Duration;

pub struct PubSubService {
    thread_hdl: JoinHandle<()>,
    exit: Arc<AtomicBool>,
    rpc_bank: Arc<RwLock<RpcPubSubBank>>,
    subscription: Arc<RpcSubscriptions>,
}

impl Service for PubSubService {
    type JoinReturnType = ();

    fn join(self) -> thread::Result<()> {
        self.thread_hdl.join()
    }
}

impl PubSubService {
    pub fn new(bank: &Arc<Bank>, pubsub_addr: SocketAddr) -> Self {
        info!("rpc_pubsub bound to {:?}", pubsub_addr);
        let rpc_bank = Arc::new(RwLock::new(RpcPubSubBank::new(bank.clone())));
        let rpc = RpcSolPubSubImpl::new(rpc_bank.clone());
        let subscription = rpc.subscription.clone();
        bank.set_subscriptions(subscription.clone());
        let exit = Arc::new(AtomicBool::new(false));
        let exit_ = exit.clone();
        let thread_hdl = Builder::new()
            .name("solana-pubsub".to_string())
            .spawn(move || {
                let mut io = PubSubHandler::default();
                io.extend_with(rpc.to_delegate());

                let server = ServerBuilder::with_meta_extractor(io, |context: &RequestContext| {
                        info!("New pubsub connection");
                        let session = Arc::new(Session::new(context.sender().clone()));
                        session.on_drop(|| {
                            info!("Pubsub connection dropped");
                        });
                        session
                })
                .start(&pubsub_addr);

                if let Err(e) = server {
                    warn!("Pubsub service unavailable error: {:?}. \nAlso, check that port {} is not already in use by another application", e, pubsub_addr.port());
                    return;
                }
                while !exit_.load(Ordering::Relaxed) {
                    sleep(Duration::from_millis(100));
                }
                server.unwrap().close();
            })
            .unwrap();
        PubSubService {
            thread_hdl,
            exit,
            rpc_bank,
            subscription,
        }
    }

    pub fn set_bank(&self, bank: &Arc<Bank>) {
        self.rpc_bank.write().unwrap().bank = bank.clone();
        bank.set_subscriptions(self.subscription.clone());
    }

    pub fn exit(&self) {
        self.exit.store(true, Ordering::Relaxed);
    }

    pub fn close(self) -> thread::Result<()> {
        self.exit();
        self.join()
    }
}

#[rpc]
pub trait RpcSolPubSub {
    type Metadata;

    // Get notification every time account userdata is changed
    // Accepts pubkey parameter as base-58 encoded string
    #[pubsub(
        subscription = "accountNotification",
        subscribe,
        name = "accountSubscribe"
    )]
    fn account_subscribe(&self, _: Self::Metadata, _: Subscriber<Account>, _: String);

    // Unsubscribe from account notification subscription.
    #[pubsub(
        subscription = "accountNotification",
        unsubscribe,
        name = "accountUnsubscribe"
    )]
    fn account_unsubscribe(&self, _: Option<Self::Metadata>, _: SubscriptionId) -> Result<bool>;

    // Get notification when signature is verified
    // Accepts signature parameter as base-58 encoded string
    #[pubsub(
        subscription = "signatureNotification",
        subscribe,
        name = "signatureSubscribe"
    )]
    fn signature_subscribe(&self, _: Self::Metadata, _: Subscriber<RpcSignatureStatus>, _: String);

    // Unsubscribe from signature notification subscription.
    #[pubsub(
        subscription = "signatureNotification",
        unsubscribe,
        name = "signatureUnsubscribe"
    )]
    fn signature_unsubscribe(&self, _: Option<Self::Metadata>, _: SubscriptionId) -> Result<bool>;
}

struct RpcPubSubBank {
    bank: Arc<Bank>,
}

impl RpcPubSubBank {
    pub fn new(bank: Arc<Bank>) -> Self {
        RpcPubSubBank { bank }
    }
}

struct RpcSolPubSubImpl {
    uid: Arc<atomic::AtomicUsize>,
    bank: Arc<RwLock<RpcPubSubBank>>,
    subscription: Arc<RpcSubscriptions>,
}

impl RpcSolPubSubImpl {
    fn new(bank: Arc<RwLock<RpcPubSubBank>>) -> Self {
        RpcSolPubSubImpl {
            uid: Arc::new(atomic::AtomicUsize::default()),
            bank,
            subscription: Arc::new(RpcSubscriptions::default()),
        }
    }

    fn subscribe_to_account_updates(&self, subscriber: Subscriber<Account>, pubkey_str: String) {
        let pubkey_vec = bs58::decode(pubkey_str).into_vec().unwrap();
        if pubkey_vec.len() != mem::size_of::<Pubkey>() {
            subscriber
                .reject(Error {
                    code: ErrorCode::InvalidParams,
                    message: "Invalid Request: Invalid pubkey provided".into(),
                    data: None,
                })
                .unwrap();
            return;
        }
        let pubkey = Pubkey::new(&pubkey_vec);

        let id = self.uid.fetch_add(1, atomic::Ordering::SeqCst);
        let sub_id = SubscriptionId::Number(id as u64);
        info!("account_subscribe: account={:?} id={:?}", pubkey, sub_id);
        let sink = subscriber.assign_id(sub_id.clone()).unwrap();

        self.subscription
            .add_account_subscription(&pubkey, &sub_id, &sink)
    }

    fn subscribe_to_signature_updates(
        &self,
        subscriber: Subscriber<RpcSignatureStatus>,
        signature_str: String,
    ) {
        info!("signature_subscribe");
        let signature_vec = bs58::decode(signature_str).into_vec().unwrap();
        if signature_vec.len() != mem::size_of::<Signature>() {
            subscriber
                .reject(Error {
                    code: ErrorCode::InvalidParams,
                    message: "Invalid Request: Invalid signature provided".into(),
                    data: None,
                })
                .unwrap();
            return;
        }
        let signature = Signature::new(&signature_vec);
        let id = self.uid.fetch_add(1, atomic::Ordering::SeqCst);
        let sub_id = SubscriptionId::Number(id as u64);
        let sink = subscriber.assign_id(sub_id.clone()).unwrap();

        let status = self
            .bank
            .read()
            .unwrap()
            .bank
            .get_signature_status(&signature);
        if status.is_none() {
            self.subscription
                .add_signature_subscription(&signature, &sub_id, &sink);
            return;
        }

        match status.unwrap() {
            Ok(_) => {
                sink.notify(Ok(RpcSignatureStatus::Confirmed))
                    .wait()
                    .unwrap();
            }
            _ => self
                .subscription
                .add_signature_subscription(&signature, &sub_id, &sink),
        }
    }
}

impl RpcSolPubSub for RpcSolPubSubImpl {
    type Metadata = Arc<Session>;

    fn account_subscribe(
        &self,
        _meta: Self::Metadata,
        subscriber: Subscriber<Account>,
        pubkey_str: String,
    ) {
        self.subscribe_to_account_updates(subscriber, pubkey_str)
    }

    fn account_unsubscribe(
        &self,
        _meta: Option<Self::Metadata>,
        id: SubscriptionId,
    ) -> Result<bool> {
        info!("account_unsubscribe: id={:?}", id);
        if self.subscription.remove_account_subscription(&id) {
            Ok(true)
        } else {
            Err(Error {
                code: ErrorCode::InvalidParams,
                message: "Invalid Request: Subscription id does not exist".into(),
                data: None,
            })
        }
    }

    fn signature_subscribe(
        &self,
        _meta: Self::Metadata,
        subscriber: Subscriber<RpcSignatureStatus>,
        signature_str: String,
    ) {
        self.subscribe_to_signature_updates(subscriber, signature_str)
    }

    fn signature_unsubscribe(
        &self,
        _meta: Option<Self::Metadata>,
        id: SubscriptionId,
    ) -> Result<bool> {
        info!("signature_unsubscribe");
        if self.subscription.remove_signature_subscription(&id) {
            Ok(true)
        } else {
            Err(Error {
                code: ErrorCode::InvalidParams,
                message: "Invalid Request: Subscription id does not exist".into(),
                data: None,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::genesis_block::GenesisBlock;
    use jsonrpc_core::futures::sync::mpsc;
    use jsonrpc_core::Response;
    use solana_sdk::budget_program;
    use solana_sdk::budget_transaction::BudgetTransaction;
    use solana_sdk::signature::{Keypair, KeypairUtil};
    use solana_sdk::system_transaction::SystemTransaction;
    use std::net::{IpAddr, Ipv4Addr};
    use tokio::prelude::{Async, Stream};

    #[test]
    fn test_pubsub_new() {
        let (genesis_block, _) = GenesisBlock::new(10_000);
        let bank = Bank::new(&genesis_block);
        let pubsub_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 0);
        let pubsub_service = PubSubService::new(&Arc::new(bank), pubsub_addr);
        let thread = pubsub_service.thread_hdl.thread();
        assert_eq!(thread.name().unwrap(), "solana-pubsub");
    }

    #[test]
    fn test_signature_subscribe() {
        let (genesis_block, alice) = GenesisBlock::new(10_000);
        let bob = Keypair::new();
        let bob_pubkey = bob.pubkey();
        let bank = Bank::new(&genesis_block);
        let arc_bank = Arc::new(bank);
        let last_id = arc_bank.last_id();

        let rpc_bank = Arc::new(RwLock::new(RpcPubSubBank::new(arc_bank.clone())));
        let rpc = RpcSolPubSubImpl::new(rpc_bank.clone());
        let subscription = rpc.subscription.clone();
        arc_bank.set_subscriptions(subscription);

        // Test signature subscription
        let tx = SystemTransaction::new_move(&alice, bob_pubkey, 20, last_id, 0);

        let (subscriber, _id_receiver, mut receiver) =
            Subscriber::new_test("signatureNotification");
        rpc.subscribe_to_signature_updates(subscriber, tx.signatures[0].to_string());

        arc_bank
            .process_transaction(&tx)
            .expect("process transaction");
        sleep(Duration::from_millis(200));

        // Test signature confirmation notification
        let string = receiver.poll();
        if let Async::Ready(Some(response)) = string.unwrap() {
            let expected = format!(r#"{{"jsonrpc":"2.0","method":"signatureNotification","params":{{"result":"Confirmed","subscription":0}}}}"#);
            assert_eq!(expected, response);
        }
    }

    #[test]
    fn test_signature_unsubscribe() {
        let (genesis_block, alice) = GenesisBlock::new(10_000);
        let bob_pubkey = Keypair::new().pubkey();
        let bank = Bank::new(&genesis_block);
        let arc_bank = Arc::new(bank);
        let last_id = arc_bank.last_id();

        let (sender, _receiver) = mpsc::channel(1);
        let session = Arc::new(Session::new(sender));

        let mut io = PubSubHandler::default();
        let rpc_bank = Arc::new(RwLock::new(RpcPubSubBank::new(arc_bank.clone())));
        let rpc = RpcSolPubSubImpl::new(rpc_bank.clone());
        io.extend_with(rpc.to_delegate());

        let tx = SystemTransaction::new_move(&alice, bob_pubkey, 20, last_id, 0);
        let req = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"signatureSubscribe","params":["{}"]}}"#,
            tx.signatures[0].to_string()
        );
        let _res = io.handle_request_sync(&req, session.clone());

        let req =
            format!(r#"{{"jsonrpc":"2.0","id":1,"method":"signatureUnsubscribe","params":[0]}}"#);
        let res = io.handle_request_sync(&req, session.clone());

        let expected = format!(r#"{{"jsonrpc":"2.0","result":true,"id":1}}"#);
        let expected: Response =
            serde_json::from_str(&expected).expect("expected response deserialization");

        let result: Response = serde_json::from_str(&res.expect("actual response"))
            .expect("actual response deserialization");
        assert_eq!(expected, result);

        // Test bad parameter
        let req =
            format!(r#"{{"jsonrpc":"2.0","id":1,"method":"signatureUnsubscribe","params":[1]}}"#);
        let res = io.handle_request_sync(&req, session.clone());
        let expected = format!(r#"{{"jsonrpc":"2.0","error":{{"code":-32602,"message":"Invalid Request: Subscription id does not exist"}},"id":1}}"#);
        let expected: Response =
            serde_json::from_str(&expected).expect("expected response deserialization");

        let result: Response = serde_json::from_str(&res.expect("actual response"))
            .expect("actual response deserialization");
        assert_eq!(expected, result);
    }

    #[test]
    fn test_account_subscribe() {
        let (genesis_block, alice) = GenesisBlock::new(10_000);
        let bob_pubkey = Keypair::new().pubkey();
        let witness = Keypair::new();
        let contract_funds = Keypair::new();
        let contract_state = Keypair::new();
        let budget_program_id = budget_program::id();
        let executable = false; // TODO
        let bank = Bank::new(&genesis_block);
        let arc_bank = Arc::new(bank);
        let last_id = arc_bank.last_id();

        let rpc_bank = Arc::new(RwLock::new(RpcPubSubBank::new(arc_bank.clone())));
        let rpc = RpcSolPubSubImpl::new(rpc_bank.clone());
        let subscription = rpc.subscription.clone();
        arc_bank.set_subscriptions(subscription);

        let (subscriber, _id_receiver, mut receiver) = Subscriber::new_test("accountNotification");
        rpc.subscribe_to_account_updates(subscriber, contract_state.pubkey().to_string());

        let tx = SystemTransaction::new_program_account(
            &alice,
            contract_funds.pubkey(),
            last_id,
            50,
            0,
            budget_program_id,
            0,
        );
        arc_bank
            .process_transaction(&tx)
            .expect("process transaction");

        let tx = SystemTransaction::new_program_account(
            &alice,
            contract_state.pubkey(),
            last_id,
            1,
            196,
            budget_program_id,
            0,
        );

        arc_bank
            .process_transaction(&tx)
            .expect("process transaction");

        // Test signature confirmation notification #1
        let string = receiver.poll();

        let expected_userdata = arc_bank
            .get_account(&contract_state.pubkey())
            .unwrap()
            .userdata;

        let expected = json!({
           "jsonrpc": "2.0",
           "method": "accountNotification",
           "params": {
               "result": {
                   "owner": budget_program_id,
                   "tokens": 1,
                   "userdata": expected_userdata,
                   "executable": executable,

               },
               "subscription": 0,
           }
        });

        if let Async::Ready(Some(response)) = string.unwrap() {
            assert_eq!(serde_json::to_string(&expected).unwrap(), response);
        }

        let tx = BudgetTransaction::new_when_signed(
            &contract_funds,
            bob_pubkey,
            contract_state.pubkey(),
            witness.pubkey(),
            None,
            50,
            last_id,
        );
        arc_bank
            .process_transaction(&tx)
            .expect("process transaction");
        sleep(Duration::from_millis(200));

        // Test signature confirmation notification #2
        let string = receiver.poll();
        let expected_userdata = arc_bank
            .get_account(&contract_state.pubkey())
            .unwrap()
            .userdata;
        let expected = json!({
           "jsonrpc": "2.0",
           "method": "accountNotification",
           "params": {
               "result": {
                   "owner": budget_program_id,
                   "tokens": 51,
                   "userdata": expected_userdata,
                    "executable": executable,
               },
               "subscription": 0,
           }
        });

        if let Async::Ready(Some(response)) = string.unwrap() {
            assert_eq!(serde_json::to_string(&expected).unwrap(), response);
        }

        let tx = SystemTransaction::new_account(&alice, witness.pubkey(), 1, last_id, 0);
        arc_bank
            .process_transaction(&tx)
            .expect("process transaction");
        sleep(Duration::from_millis(200));
        let tx = BudgetTransaction::new_signature(
            &witness,
            contract_state.pubkey(),
            bob_pubkey,
            last_id,
        );
        arc_bank
            .process_transaction(&tx)
            .expect("process transaction");
        sleep(Duration::from_millis(200));

        let expected_userdata = arc_bank
            .get_account(&contract_state.pubkey())
            .unwrap()
            .userdata;
        let expected = json!({
           "jsonrpc": "2.0",
           "method": "accountNotification",
           "params": {
               "result": {
                   "owner": budget_program_id,
                   "tokens": 1,
                   "userdata": expected_userdata,
                    "executable": executable,
               },
               "subscription": 0,
           }
        });
        let string = receiver.poll();
        if let Async::Ready(Some(response)) = string.unwrap() {
            assert_eq!(serde_json::to_string(&expected).unwrap(), response);
        }
    }

    #[test]
    fn test_account_unsubscribe() {
        let (genesis_block, _) = GenesisBlock::new(10_000);
        let bob_pubkey = Keypair::new().pubkey();
        let bank = Bank::new(&genesis_block);
        let arc_bank = Arc::new(bank);

        let (sender, _receiver) = mpsc::channel(1);
        let session = Arc::new(Session::new(sender));

        let mut io = PubSubHandler::default();
        let rpc_bank = Arc::new(RwLock::new(RpcPubSubBank::new(arc_bank.clone())));
        let rpc = RpcSolPubSubImpl::new(rpc_bank.clone());

        io.extend_with(rpc.to_delegate());

        let req = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"accountSubscribe","params":["{}"]}}"#,
            bob_pubkey.to_string()
        );
        let _res = io.handle_request_sync(&req, session.clone());

        let req =
            format!(r#"{{"jsonrpc":"2.0","id":1,"method":"accountUnsubscribe","params":[0]}}"#);
        let res = io.handle_request_sync(&req, session.clone());

        let expected = format!(r#"{{"jsonrpc":"2.0","result":true,"id":1}}"#);
        let expected: Response =
            serde_json::from_str(&expected).expect("expected response deserialization");

        let result: Response = serde_json::from_str(&res.expect("actual response"))
            .expect("actual response deserialization");
        assert_eq!(expected, result);

        // Test bad parameter
        let req =
            format!(r#"{{"jsonrpc":"2.0","id":1,"method":"accountUnsubscribe","params":[1]}}"#);
        let res = io.handle_request_sync(&req, session.clone());
        let expected = format!(r#"{{"jsonrpc":"2.0","error":{{"code":-32602,"message":"Invalid Request: Subscription id does not exist"}},"id":1}}"#);
        let expected: Response =
            serde_json::from_str(&expected).expect("expected response deserialization");

        let result: Response = serde_json::from_str(&res.expect("actual response"))
            .expect("actual response deserialization");
        assert_eq!(expected, result);
    }
}
