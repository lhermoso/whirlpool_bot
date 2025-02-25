use solana_client::nonblocking::rpc_client::RpcClient;
use anyhow::Result;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::account::Account;
use std::sync::Arc;

pub struct SolanaRpcClient {
    pub rpc: Arc<RpcClient>,
}

impl SolanaRpcClient {
    pub fn new(url: &str) -> Result<Self> {
        Ok(Self {
            rpc: Arc::new(RpcClient::new(url.to_string())),
        })
    }

    #[allow(dead_code)]
    pub async fn get_account_data(&self, pubkey: Pubkey) -> Result<Account> {
        self.rpc.get_account(&pubkey).await
            .map_err(|e| anyhow::anyhow!("Failed to fetch account data: {}", e))
    }
}

impl Clone for SolanaRpcClient {
    fn clone(&self) -> Self {
        Self {
            rpc: self.rpc.clone(),
        }
    }
}