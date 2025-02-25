use solana_client::nonblocking::rpc_client::RpcClient;
use anyhow::Result;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::account::Account; // Correct import for Account

pub struct SolanaRpcClient {
    pub rpc: RpcClient,
}

impl SolanaRpcClient {
    pub fn new(url: &str) -> Result<Self> {
        Ok(Self {
            rpc: RpcClient::new(url.to_string()),
        })
    }

    pub async fn get_account_data(&self, pubkey: Pubkey) -> Result<Account> { // Use Account directly
        self.rpc.get_account(&pubkey).await
            .map_err(|e| anyhow::anyhow!("Failed to fetch account data: {}", e))
    }
}

// Remove Clone implementation since RpcClient isn't clonable
// impl Clone for SolanaRpcClient {
//     fn clone(&self) -> Self {
//         Self {
//             rpc: self.rpc.clone(),
//         }
//     }
// }