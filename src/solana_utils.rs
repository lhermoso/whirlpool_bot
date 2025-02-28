use solana_client::nonblocking::rpc_client::RpcClient;
use anyhow::Result;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::account::Account;
use std::sync::Arc;
use solana_sdk::commitment_config::{CommitmentConfig, CommitmentLevel};
use solana_sdk::transaction::Transaction;
use solana_sdk::signature::Signature;
use solana_client::rpc_config::RpcSendTransactionConfig;


/// `whirlpool` program ID.


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
    
    /// Send and confirm a transaction with the specified commitment level
    pub async fn send_and_confirm_transaction_with_commitment(
        &self,
        transaction: &Transaction,
        commitment: CommitmentLevel,
    ) -> Result<Signature> {
        let config = RpcSendTransactionConfig {
            skip_preflight: false,
            preflight_commitment: Some(commitment),
            encoding: None,
            max_retries: Some(3),
            min_context_slot: None,
        };
        
        let signature = self.rpc.send_transaction_with_config(transaction, config).await
            .map_err(|e| anyhow::anyhow!("Failed to send transaction: {}", e))?;
            
        // Wait for confirmation with the specified commitment level
        let commitment_config = CommitmentConfig { commitment };
        self.rpc.confirm_transaction_with_commitment(&signature, commitment_config).await
            .map_err(|e| anyhow::anyhow!("Failed to confirm transaction: {}", e))?;
            
        Ok(signature)
    }
    
    /// Send a transaction with configuration and return immediately without waiting for confirmation
    pub async fn send_transaction_with_config(
        &self,
        transaction: &Transaction,
        config: RpcSendTransactionConfig,
    ) -> Result<Signature> {
        self.rpc.send_transaction_with_config(transaction, config).await
            .map_err(|e| anyhow::anyhow!("Failed to send transaction: {}", e))
    }
}

impl Clone for SolanaRpcClient {
    fn clone(&self) -> Self {
        Self {
            rpc: self.rpc.clone(),
        }
    }
}