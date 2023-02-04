use {
    crate::connection_cache::ConnectionCache,
    solana_connection_cache::connection_cache::ConnectionCache as BackendConnectionCache,
    solana_rpc_client::rpc_client::RpcClient,
    solana_sdk::{
        message::Message,
        signers::Signers,
        transaction::{Transaction, TransactionError},
        transport::Result as TransportResult,
    },
    solana_tpu_client::tpu_client::{temporary_pub::Result, TpuClient as BackendTpuClient},
    std::sync::Arc,
};
pub use {
    crate::nonblocking::tpu_client::TpuSenderError,
    solana_tpu_client::tpu_client::{TpuClientConfig, DEFAULT_FANOUT_SLOTS, MAX_FANOUT_SLOTS},
};

/// Client which sends transactions directly to the current leader's TPU port over UDP.
/// The client uses RPC to determine the current leader and fetch node contact info
/// This is just a thin wrapper over the "BackendTpuClient", use that directly for more efficiency.
pub struct TpuClient {
    tpu_client: BackendTpuClient,
}

impl TpuClient {
    /// Serialize and send transaction to the current and upcoming leader TPUs according to fanout
    /// size
    pub fn send_transaction(&self, transaction: &Transaction) -> bool {
        self.tpu_client.send_transaction(transaction)
    }

    /// Send a wire transaction to the current and upcoming leader TPUs according to fanout size
    pub fn send_wire_transaction(&self, wire_transaction: Vec<u8>) -> bool {
        self.tpu_client.send_wire_transaction(wire_transaction)
    }

    /// Serialize and send transaction to the current and upcoming leader TPUs according to fanout
    /// size
    /// Returns the last error if all sends fail
    pub fn try_send_transaction(&self, transaction: &Transaction) -> TransportResult<()> {
        self.tpu_client.try_send_transaction(transaction)
    }

    /// Serialize and send a batch of transactions to the current and upcoming leader TPUs according
    /// to fanout size
    /// Returns the last error if all sends fail
    pub fn try_send_transaction_batch(&self, transactions: &[Transaction]) -> TransportResult<()> {
        self.tpu_client.try_send_transaction_batch(transactions)
    }

    /// Send a wire transaction to the current and upcoming leader TPUs according to fanout size
    /// Returns the last error if all sends fail
    pub fn try_send_wire_transaction(&self, wire_transaction: Vec<u8>) -> TransportResult<()> {
        self.tpu_client.try_send_wire_transaction(wire_transaction)
    }

    /// Create a new client that disconnects when dropped
    pub fn new(
        rpc_client: Arc<RpcClient>,
        websocket_url: &str,
        config: TpuClientConfig,
    ) -> Result<Self> {
        let connection_cache = ConnectionCache::default();
        Self::new_with_connection_cache(
            rpc_client,
            websocket_url,
            config,
            Arc::new(connection_cache.into()),
        )
    }

    /// Create a new client that disconnects when dropped
    pub fn new_with_connection_cache(
        rpc_client: Arc<RpcClient>,
        websocket_url: &str,
        config: TpuClientConfig,
        connection_cache: Arc<BackendConnectionCache>,
    ) -> Result<Self> {
        Ok(Self {
            tpu_client: BackendTpuClient::new_with_connection_cache(
                rpc_client,
                websocket_url,
                config,
                connection_cache,
            )?,
        })
    }

    pub fn send_and_confirm_messages_with_spinner<T: Signers>(
        &self,
        messages: &[Message],
        signers: &T,
    ) -> Result<Vec<Option<TransactionError>>> {
        self.tpu_client
            .send_and_confirm_messages_with_spinner(messages, signers)
    }

    pub fn rpc_client(&self) -> &RpcClient {
        self.tpu_client.rpc_client()
    }
}
