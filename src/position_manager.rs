use orca_whirlpools::{
    open_position_instructions, close_position_instructions, swap_instructions,
    IncreaseLiquidityParam, set_funder
};
use orca_whirlpools_client::Whirlpool;
use orca_whirlpools_core::sqrt_price_to_price;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_client::rpc_request::TokenAccountsFilter;
use solana_sdk::{
    pubkey::Pubkey,
    signature::{Signature, Signer},
    transaction::Transaction,
    program_pack::Pack,
    commitment_config::CommitmentLevel,
};
use anyhow::{Result, Context};
use spl_token_2022::state::Mint;
use std::collections::HashMap;
use tokio::sync::Mutex;
use std::str::FromStr;
use std::sync::Arc;

use crate::solana_utils::SolanaRpcClient;

const PROGRAM_ID: Pubkey = Pubkey::new_from_array([
    0x6e, 0xeb, 0x65, 0x4a, 0x8e, 0x36, 0xe7, 0x49, 0xd0, 0x8f, 0xb8, 0x33, 0x5c, 0xd3, 0xa8,
    0xea, 0x6f, 0x73, 0xcc, 0x37, 0x11, 0x6f, 0x2e, 0xc1, 0x9b, 0x9e, 0x99, 0x7e, 0x58, 0x6b,
    0x9b, 0x9c,
]);

fn get_position_address(position_mint: &Pubkey) -> Result<(Pubkey, u8), anyhow::Error> {
    Ok(Pubkey::find_program_address(&[b"position", position_mint.as_ref()], &PROGRAM_ID))
}

pub async fn fetch_mint(rpc: &Arc<RpcClient>, mint_address: &Pubkey, cache: &Mutex<HashMap<Pubkey, Mint>>) -> Result<Mint> {
    let mut cache_lock = cache.lock().await;
    if let Some(mint) = cache_lock.get(mint_address) {
        return Ok(mint.clone());
    }

    let mint_account = rpc
        .get_account(mint_address)
        .await
        .with_context(|| format!("Failed to fetch account data for mint: {}", mint_address))?;

    let mint = Mint::unpack(&mint_account.data)
        .with_context(|| format!("Failed to unpack mint data for: {}", mint_address))?;

    cache_lock.insert(*mint_address, mint.clone());
    Ok(mint)
}

#[derive(Clone)]
pub struct Position {
    pub lower_price: f64,
    pub upper_price: f64,
    pub mint_address: Pubkey,
    #[allow(dead_code)]
    pub address: Pubkey,
    pub tick_lower_index: i32,
    pub tick_upper_index: i32,
    #[allow(dead_code)]
    pub liquidity: u128,
}

pub struct PositionManager {
    pub client: SolanaRpcClient,
    wallet: Box<dyn Signer>,
    pool_address: Pubkey,
    position: Option<Position>,
    rpc: Arc<RpcClient>,
    token_mint_a: Pubkey,
    token_mint_b: Pubkey,
    token_mint_a_decimals: u8,
    token_mint_b_decimals: u8,
    #[allow(dead_code)]
    mint_cache: Mutex<HashMap<Pubkey, Mint>>,
}

impl PositionManager {
    pub async fn new(client: SolanaRpcClient, wallet: Box<dyn Signer>, pool_address: Pubkey) -> Result<Self> {
        set_funder(wallet.pubkey())
            .map_err(|e| anyhow::anyhow!("Failed to set funder: {}", e))?;
        let rpc = client.rpc.clone();

        // Get pool account with better error handling
        println!("Fetching whirlpool account data for {}", pool_address);
        let whirlpool_account = client.rpc.get_account(&pool_address).await
            .map_err(|e| anyhow::anyhow!("Failed to fetch whirlpool account: {}", e))?;
        
        println!("Account data size: {} bytes", whirlpool_account.data.len());
        
        // Verify this is actually a Whirlpool account
        let whirlpool = match Whirlpool::from_bytes(&whirlpool_account.data) {
            Ok(w) => {
                println!("Successfully deserialized Whirlpool data");
                w
            },
            Err(e) => {
                println!("Error deserializing Whirlpool: {}", e);
                println!("This likely means the address doesn't point to a valid Whirlpool pool");
                println!("Account owner: {}", whirlpool_account.owner);
                println!("Expected Whirlpool Program ID: {}", PROGRAM_ID);
                return Err(anyhow::anyhow!("Failed to deserialize Whirlpool data: {}. Check that this address is a valid Whirlpool pool on the selected network.", e));
            }
        };

        println!("Fetching token mint info");
        let mint_cache = Mutex::new(HashMap::new());
        
        // Fetch token mint A with error handling
        println!("Fetching token mint A: {}", whirlpool.token_mint_a);
        let token_mint_a = match fetch_mint(&rpc, &whirlpool.token_mint_a, &mint_cache).await {
            Ok(mint) => {
                println!("Token A mint fetched successfully: decimals={}", mint.decimals);
                mint
            },
            Err(e) => {
                println!("Error fetching token A mint: {}", e);
                return Err(anyhow::anyhow!("Failed to fetch token A mint: {}", e));
            }
        };
        
        // Fetch token mint B with error handling
        println!("Fetching token mint B: {}", whirlpool.token_mint_b);
        let token_mint_b = match fetch_mint(&rpc, &whirlpool.token_mint_b, &mint_cache).await {
            Ok(mint) => {
                println!("Token B mint fetched successfully: decimals={}", mint.decimals);
                mint
            },
            Err(e) => {
                println!("Error fetching token B mint: {}", e);
                return Err(anyhow::anyhow!("Failed to fetch token B mint: {}", e));
            }
        };

        Ok(PositionManager {
            client,
            wallet,
            pool_address,
            position: None,
            rpc,
            token_mint_a: whirlpool.token_mint_a,
            token_mint_b: whirlpool.token_mint_b,
            token_mint_a_decimals: token_mint_a.decimals,
            token_mint_b_decimals: token_mint_b.decimals,
            mint_cache,
        })
    }

    pub async fn get_current_price(&self) -> Result<f64> {
        let whirlpool_account = self.client.rpc.get_account(&self.pool_address).await?;
        let whirlpool = Whirlpool::from_bytes(&whirlpool_account.data)
            .map_err(|e| anyhow::anyhow!("Failed to deserialize Whirlpool: {}", e))?;
        Ok(sqrt_price_to_price(
            whirlpool.sqrt_price,
            self.token_mint_a_decimals,
            self.token_mint_b_decimals,
        ))
    }

    pub async fn get_whirlpool(&self) -> Result<Whirlpool> {
        let whirlpool_account = self.client.rpc.get_account(&self.pool_address).await?;
        Whirlpool::from_bytes(&whirlpool_account.data)
            .map_err(|e| anyhow::anyhow!("Failed to deserialize Whirlpool: {}", e))
    }

    pub async fn get_balance(&self, token_mint: Pubkey) -> Result<u64> {
        // Special case for SOL (native token)
        let sol_mint = Pubkey::from_str("So11111111111111111111111111111111111111112")?;
        
        if token_mint == sol_mint {
            // Check native SOL balance directly
            return self.get_native_sol_balance().await;
        }
        
        // For other tokens, check token accounts
        let token_accounts = self.client.rpc.get_token_accounts_by_owner(&self.wallet.pubkey(), TokenAccountsFilter::Mint(token_mint)).await?;
        if let Some(account) = token_accounts.first() {
            let account_pubkey = Pubkey::from_str(&account.pubkey)?;
            let balance = self.client.rpc.get_token_account_balance(&account_pubkey).await?;
            Ok(balance.amount.parse::<u64>()?)
        } else {
            println!("No token account found for mint: {}", token_mint);
            Ok(0)
        }
    }

    pub async fn swap_tokens(&self, amount: u64, from_mint: Pubkey, to_mint: Pubkey) -> Result<Signature> {
        use solana_sdk::compute_budget::ComputeBudgetInstruction;
        use solana_sdk::message::Message;
        use solana_client::rpc_config::RpcSendTransactionConfig;
        use solana_sdk::commitment_config::CommitmentLevel;
        use tokio::time::{sleep, Duration, Instant};
        
        let slippage_tolerance = Some(100u16); // 1%
        println!("Preparing to swap {} from {} to {}", 
            amount as f64 / 1_000_000_000.0, from_mint, to_mint);
        
        // Ensure token accounts exist before attempting swap
        if to_mint != Pubkey::from_str("So11111111111111111111111111111111111111112").unwrap() {
            println!("Verifying destination token account exists before swap...");
            match self.create_token_account_if_needed(to_mint).await {
                Ok(_) => {
                    // Verify that the token account is fully initialized by checking its existence
                    let token_accounts = self.client.rpc.get_token_accounts_by_owner(
                        &self.wallet.pubkey(),
                        TokenAccountsFilter::Mint(to_mint)
                    ).await;
                    
                    match token_accounts {
                        Ok(accounts) if !accounts.is_empty() => {
                            println!("Confirmed token account exists for destination token: {}", to_mint);
                            // Add a small delay to ensure blockchain state is updated
                            sleep(Duration::from_millis(2000)).await;
                        },
                        Ok(_) => {
                            println!("Warning: Couldn't find token account after creation. Creating again...");
                            self.create_token_account_if_needed(to_mint).await?;
                            sleep(Duration::from_millis(3000)).await;
                        },
                        Err(e) => {
                            println!("Error verifying token account: {}", e);
                            return Err(anyhow::anyhow!("Failed to verify token account: {}", e));
                        }
                    }
                },
                Err(e) => {
                    println!("Failed to create destination token account: {}", e);
                    return Err(anyhow::anyhow!("Failed to create destination token account: {}", e));
                }
            }
        }
        
        // 1. Get swap instructions from SDK
        println!("Retrieving swap instructions...");
        let swap_result = swap_instructions(
            &self.client.rpc,
            self.pool_address,
            amount,
            from_mint,
            orca_whirlpools::SwapType::ExactIn,
            slippage_tolerance,
            Some(self.wallet.pubkey()),
        ).await.map_err(|e| anyhow::anyhow!("Swap instruction error: {}", e))?;

        println!("Got swap instructions. Number of instructions: {}", swap_result.instructions.len());
        println!("Quote estimated token out: {:?}", swap_result.quote);
        
        // Create message from instructions
        let message = Message::new(
            &swap_result.instructions,
            Some(&self.wallet.pubkey()),
        );
        
        // 2. Prepare signers
        let mut signers: Vec<&dyn Signer> = vec![self.wallet.as_ref()];
        
        // Add any additional signers from the SDK result
        signers.extend(
            swap_result.additional_signers.iter()
                .map(|kp| kp as &dyn Signer),
        );
        
        println!("Using {} signers for transaction", signers.len());
        
        // Set up retry parameters
        let max_retries = 5;
        let base_backoff_ms = 1000; // Start with 1 second
        let max_backoff_ms = 15000; // Max 15 seconds
        let transaction_timeout = Duration::from_secs(90);
        
        for attempt in 0..max_retries {
            if attempt > 0 {
                println!("Retry attempt {}/{} for swap transaction", attempt, max_retries);
            }
            
            // 3. Get latest blockhash
            let recent_blockhash = self.client.rpc.get_latest_blockhash().await?;
            
            // 4. Create transaction for simulation
            let transaction = Transaction::new(&signers, message.clone(), recent_blockhash);
            
            // 5. Simulate transaction to get compute units
            println!("Simulating transaction to determine compute units...");
            let simulated_transaction = match self.client.rpc.simulate_transaction(&transaction).await {
                Ok(sim) => {
                    if let Some(err) = &sim.value.err {
                        println!("Transaction simulation failed: {:?}", err);
                        println!("Logs: {:?}", sim.value.logs);
                        
                        // Check for specific errors that warrant retry
                        let err_str = format!("{:?}", err);
                        if err_str.contains("429") || err_str.contains("Too Many Requests") {
                            let backoff = Self::calculate_backoff(attempt, base_backoff_ms, max_backoff_ms);
                            println!("Rate limit hit during simulation. Retrying in {}ms", backoff);
                            sleep(Duration::from_millis(backoff)).await;
                            continue;
                        }
                        
                        // Some errors mean token account needs to be created
                        if err_str.contains("Account does not exist") && 
                           to_mint != Pubkey::from_str("So11111111111111111111111111111111111111112")? {
                            println!("Token account may not exist. Creating token account for {}", to_mint);
                            match self.create_token_account_if_needed(to_mint).await {
                                Ok(_) => {
                                    println!("Token account created or verified. Retrying swap...");
                                    // Apply a short wait after account creation
                                    sleep(Duration::from_millis(1000)).await;
                                    continue;
                                },
                                Err(create_err) => {
                                    println!("Failed to create token account: {}", create_err);
                                    // Apply backoff and retry from the beginning
                                    let backoff = Self::calculate_backoff(attempt, base_backoff_ms, max_backoff_ms);
                                    sleep(Duration::from_millis(backoff)).await;
                                    continue;
                                }
                            }
                        }
                        
                        return Err(anyhow::anyhow!("Transaction simulation failed: {:?}", err));
                    }
                    sim
                },
                Err(e) => {
                    // Check if this is a rate limit error
                    if e.to_string().contains("429") || e.to_string().contains("Too Many Requests") {
                        let backoff = Self::calculate_backoff(attempt, base_backoff_ms, max_backoff_ms);
                        println!("Rate limit hit during simulation. Retrying in {}ms", backoff);
                        sleep(Duration::from_millis(backoff)).await;
                        continue;
                    }
                    
                    println!("Failed to simulate transaction: {}", e);
                    // For other errors, use a shorter backoff
                    sleep(Duration::from_millis((attempt as u64 + 1) * 500)).await;
                    continue;
                }
            };
            
            // 6. Create final transaction with compute budget instructions
            let mut all_instructions = vec![];
            
            // Add compute budget instructions if simulation provided units consumed
            if let Some(units_consumed) = simulated_transaction.value.units_consumed {
                let units_consumed_safe = units_consumed as u32 + 100_000;
                println!("Adding compute budget limit: {} units", units_consumed_safe);
                let compute_limit_instruction = 
                    ComputeBudgetInstruction::set_compute_unit_limit(units_consumed_safe);
                all_instructions.push(compute_limit_instruction);
                
                // Add prioritization fees based on recent fees for this pool
                let prioritization_fees = match self.client.rpc.get_recent_prioritization_fees(&[self.pool_address]).await {
                    Ok(fees) => fees,
                    Err(e) => {
                        println!("Failed to get prioritization fees, continuing without them: {}", e);
                        vec![]
                    }
                };
                
                if !prioritization_fees.is_empty() {
                    let mut prioritization_fees_array: Vec<u64> = prioritization_fees
                        .iter()
                        .map(|fee| fee.prioritization_fee)
                        .collect();
                    prioritization_fees_array.sort_unstable();
                    let prioritization_fee = prioritization_fees_array
                        .get(prioritization_fees_array.len() / 2)
                        .cloned();
                        
                    if let Some(fee) = prioritization_fee {
                        println!("Adding prioritization fee: {} micro-lamports per CU", fee);
                        let priority_fee_instruction = 
                            ComputeBudgetInstruction::set_compute_unit_price(fee);
                        all_instructions.push(priority_fee_instruction);
                    }
                }
            }
            
            // Add the swap instructions
            all_instructions.extend(swap_result.instructions.clone());
            
            // Create final message and transaction
            let final_message = Message::new(&all_instructions, Some(&self.wallet.pubkey()));
            let final_transaction = Transaction::new(&signers, final_message, recent_blockhash);
            
            // 7. Set transaction config
            let transaction_config = RpcSendTransactionConfig {
                skip_preflight: true,
                preflight_commitment: Some(CommitmentLevel::Confirmed),
                max_retries: Some(1), // We'll handle our own retries
                ..Default::default()
            };
            
            // 8. Submit transaction with timeout and polling
            println!("Submitting swap transaction...");
            let start_time = Instant::now();
            
            match self.client.send_transaction_with_config(&final_transaction, transaction_config).await {
                Ok(sig) => {
                    println!("Transaction submitted with signature: {}", sig);
                    
                    // Poll for confirmation with timeout
                    let confirmed = false;
                    while start_time.elapsed() < transaction_timeout {
                        // Wait a bit between polling attempts
                        sleep(Duration::from_millis(1000)).await;
                        
                        // Poll for confirmation
                        let status_result = self.client.rpc.get_signature_statuses(&[sig]).await;
                        match status_result {
                            Ok(response) => {
                                if let Some(Some(status)) = response.value.get(0) {
                                    if let Some(err) = &status.err {
                                        println!("Transaction failed: {:?}", err);
                                        break;
                                    } else if status.confirmation_status.is_some() {
                                        println!("Transaction confirmed!");
                                        println!("Swap transaction successful! Signature: {}", sig);
                                        println!("Swapped {} from {} to {}.", 
                                            amount as f64 / 1_000_000_000.0, from_mint, to_mint);
                                            
                                        // Wait a moment for chain state to update
                                        sleep(Duration::from_secs(2)).await;
                                        
                                        return Ok(sig);
                                    }
                                }
                            },
                            Err(e) => {
                                // Check if this is a rate limit error
                                if e.to_string().contains("429") || e.to_string().contains("Too Many Requests") {
                                    println!("Rate limit hit during status check: {}", e);
                                    // Continue polling but wait longer
                                    sleep(Duration::from_millis(2000)).await;
                                } else {
                                    println!("Failed to get transaction status: {}", e);
                                }
                            }
                        }
                    }
                    
                    if !confirmed {
                        println!("Transaction timed out after {:?}", transaction_timeout);
                        // Apply backoff before retrying
                        let backoff = Self::calculate_backoff(attempt, base_backoff_ms, max_backoff_ms);
                        sleep(Duration::from_millis(backoff)).await;
                    }
                },
                Err(e) => {
                    // Check for specific errors
                    let err_string = e.to_string();
                    
                    // Rate limiting errors
                    if err_string.contains("429") || err_string.contains("Too Many Requests") {
                        let backoff = Self::calculate_backoff(attempt, base_backoff_ms, max_backoff_ms);
                        println!("Rate limit hit (429). Retrying in {}ms", backoff);
                        sleep(Duration::from_millis(backoff)).await;
                        continue;
                    }
                    
                    // Token account doesn't exist
                    if err_string.contains("Account does not exist") && 
                       to_mint != Pubkey::from_str("So11111111111111111111111111111111111111112")? {
                        println!("Token account may not exist. Creating token account for {}", to_mint);
                        match self.create_token_account_if_needed(to_mint).await {
                            Ok(_) => {
                                println!("Token account created or verified. Retrying swap...");
                                // Apply a short wait after account creation
                                sleep(Duration::from_millis(1000)).await;
                                continue;
                            },
                            Err(create_err) => {
                                println!("Failed to create token account: {}", create_err);
                                return Err(anyhow::anyhow!("Failed to create token account: {}", create_err));
                            }
                        }
                    }
                    
                    // Other errors
                    println!("Failed to submit transaction: {}", e);
                    // Apply backoff before retrying
                    let backoff = Self::calculate_backoff(attempt, base_backoff_ms / 2, max_backoff_ms / 2);
                    sleep(Duration::from_millis(backoff)).await;
                }
            }
        }
        
        Err(anyhow::anyhow!("Failed to execute swap after {} attempts", max_retries))
    }
    
    // New helper method to create token accounts
    async fn create_token_account_if_needed(&self, mint: Pubkey) -> Result<()> {
        use spl_associated_token_account::instruction::create_associated_token_account;
        use solana_client::rpc_config::RpcSendTransactionConfig;
        use solana_sdk::commitment_config::CommitmentLevel;
        use tokio::time::{sleep, Duration, Instant};
        
        println!("Creating associated token account for mint: {}", mint);
        
        // First check if token account already exists
        let token_accounts = self.client.rpc.get_token_accounts_by_owner(
            &self.wallet.pubkey(),
            TokenAccountsFilter::Mint(mint)
        ).await;
        
        match token_accounts {
            Ok(accounts) if !accounts.is_empty() => {
                println!("Token account already exists");
                return Ok(());
            },
            Err(e) => {
                println!("Error checking for existing token account: {}", e);
                // Continue with creation attempt
            },
            _ => {} // No accounts found, continue with creation
        }
        
        // Check which token program the mint uses
        let token_program_id = match self.client.rpc.get_account(&mint).await {
            Ok(account) => {
                // Check the owner of the mint account to determine which token program it uses
                let standard_token_program = spl_token::id();
                let token_2022_program = spl_token_2022::id();
                
                if account.owner == standard_token_program {
                    println!("Mint uses standard SPL Token program");
                    standard_token_program
                } else if account.owner == token_2022_program {
                    println!("Mint uses Token-2022 program");
                    token_2022_program
                } else {
                    println!("Warning: Mint owner is neither standard Token nor Token-2022 program: {}", account.owner);
                    println!("Defaulting to standard Token program");
                    standard_token_program
                }
            },
            Err(e) => {
                println!("Could not fetch mint account, defaulting to standard Token program: {}", e);
                spl_token::id()
            }
        };
        
        let instruction = create_associated_token_account(
            &self.wallet.pubkey(), // Funding account
            &self.wallet.pubkey(), // Wallet address
            &mint,                 // Token mint
            &token_program_id,     // Use the correct token program ID
        );
        
        let max_retries = 5;
        let start_backoff_ms = 1000; // Start with 1 second
        let max_backoff_ms = 15000;  // Max 15 seconds
        
        // Transaction config with preflight disabled to avoid additional RPC calls
        let transaction_config = RpcSendTransactionConfig {
            skip_preflight: true,
            preflight_commitment: Some(CommitmentLevel::Confirmed),
            max_retries: Some(1),
            ..Default::default()
        };
        
        for attempt in 0..max_retries {
            // Get a fresh blockhash for each attempt
            let recent_blockhash = match self.client.rpc.get_latest_blockhash().await {
                Ok(hash) => hash,
                Err(e) => {
                    // If we can't get a blockhash, apply backoff and retry
                    let backoff = Self::calculate_backoff(attempt, start_backoff_ms, max_backoff_ms);
                    println!("Failed to get blockhash: {}. Retrying in {}ms (attempt {}/{})", 
                        e, backoff, attempt + 1, max_retries);
                    sleep(Duration::from_millis(backoff)).await;
                    continue;
                }
            };
            
            let tx = Transaction::new_signed_with_payer(
                &[instruction.clone()],
                Some(&self.wallet.pubkey()),
                &[self.wallet.as_ref()],
                recent_blockhash,
            );
            
            let start_time = Instant::now();
            match self.client.send_transaction_with_config(&tx, transaction_config).await {
                Ok(sig) => {
                    println!("Token account creation transaction submitted: {}", sig);
                    
                    // Poll for confirmation with timeout
                    let confirmation_timeout = Duration::from_secs(30);
                    let poll_interval = Duration::from_millis(1000);
                    let start_poll_time = Instant::now();
                    
                    let mut confirmed = false;
                    while start_poll_time.elapsed() < confirmation_timeout {
                        match self.client.rpc.get_signature_statuses(&[sig]).await {
                            Ok(response) => {
                                if let Some(Some(status)) = response.value.get(0) {
                                    if let Some(err) = &status.err {
                                        println!("Transaction failed: {:?}", err);
                                        break;
                                    } else if status.confirmation_status.is_some() {
                                        println!("Token account created successfully!");
                                        confirmed = true;
                                        break;
                                    }
                                }
                            },
                            Err(e) => {
                                println!("Error checking transaction status: {}", e);
                            }
                        }
                        sleep(poll_interval).await;
                    }
                    
                    if confirmed {
                        return Ok(());
                    } else {
                        println!("Transaction not confirmed within timeout window");
                    }
                },
                Err(e) => {
                    // Check specifically for rate limit errors
                    if e.to_string().contains("429") || e.to_string().contains("Too Many Requests") {
                        let backoff = Self::calculate_backoff(attempt, start_backoff_ms, max_backoff_ms);
                        println!("Rate limit exceeded (429). Retrying in {}ms (attempt {}/{})", 
                            backoff, attempt + 1, max_retries);
                        sleep(Duration::from_millis(backoff)).await;
                    } else if e.to_string().contains("already in use") {
                        // This means the account was actually created
                        println!("Token account already exists");
                        return Ok(());
                    } else {
                        println!("Error sending transaction: {}. Retrying...", e);
                        // Apply shorter backoff for other errors
                        sleep(Duration::from_millis(((attempt + 1) * 500).into())).await;
                    }
                }
            }
            
            // Add some base delay between attempts to avoid hammering the RPC
            let elapsed = start_time.elapsed().as_millis() as u64;
            if elapsed < 1000 {
                sleep(Duration::from_millis(1000 - elapsed)).await;
            }
        }
        
        // After all retries
        Err(anyhow::anyhow!("Failed to create token account after {} attempts", max_retries))
    }
    
    // Helper method for calculating exponential backoff with jitter
    fn calculate_backoff(attempt: u32, base_ms: u64, max_ms: u64) -> u64 {
        use rand::thread_rng;
        use rand::Rng;
        let mut rng = thread_rng();
        
        // Calculate exponential backoff: base * 2^attempt
        let backoff = base_ms * (1 << attempt);
        
        // Apply max cap
        let capped_backoff = std::cmp::min(backoff, max_ms);
        
        // Add jitter (±15%)
        let jitter_factor = rng.gen_range(0.85..1.15);
        (capped_backoff as f64 * jitter_factor) as u64
    }

    pub async fn open_position_with_balance_check(&mut self, lower_price: f64, upper_price: f64, amount: u64) -> Result<()> {
        // Use a loop instead of recursion to handle investment amount adjustments
        let mut current_amount = amount;
        let mut retry_count = 0;
        let max_retries = 3;

        loop {
            // Calculate a reasonable gas reserve based on investment amount
            let invest_percentage = 0.1; // 10% of investment amount for gas
            let min_gas_reserve = 10_000_000; // 0.01 SOL minimum
            let max_gas_reserve = 50_000_000; // 0.05 SOL maximum
            let calculated_reserve = (current_amount as f64 * invest_percentage) as u64;
            let gas_reserve = min_gas_reserve.max(calculated_reserve.min(max_gas_reserve));
            
            println!("Using gas reserve of {} SOL", gas_reserve as f64 / 1_000_000_000.0);
            
            // IMPORTANT: Subtract gas reserve from total amount to get actual investment amount
            let investment_amount = current_amount.saturating_sub(gas_reserve);
            println!("Total amount: {} SOL, Gas reserve: {} SOL, Available for investment: {} SOL", 
                current_amount as f64 / 1_000_000_000.0,
                gas_reserve as f64 / 1_000_000_000.0,
                investment_amount as f64 / 1_000_000_000.0);
            
            let sol_mint = Pubkey::from_str("So11111111111111111111111111111111111111112")?;
            
            // Get the whirlpool data first to access tick spacing
            println!("Fetching whirlpool data for tick and price calculations");
            let whirlpool = self.get_whirlpool().await?;
            let tick_spacing = whirlpool.tick_spacing as i32;
            println!("Pool tick spacing: {}", tick_spacing);
            
            // Get current price for calculations
            let current_price = self.get_current_price().await?;
            println!("Current price: {}", current_price);
            
            // Convert price to tick index and ensure it's aligned with tick spacing
            let price_to_tick_index = |price: f64| -> i32 {
                let raw_tick_index = (price.ln() / 1.0001f64.ln()) as i32;
                // Round to nearest valid tick based on spacing
                (raw_tick_index / tick_spacing) * tick_spacing
            };
            
            // Align lower and upper prices to valid ticks
            let tick_lower_index = price_to_tick_index(lower_price);
            let tick_upper_index = price_to_tick_index(upper_price);
            
            // Convert back to actual prices that correspond to valid ticks
            let actual_lower_price = 1.0001f64.powi(tick_lower_index);
            let actual_upper_price = 1.0001f64.powi(tick_upper_index);
            
            println!("Aligned price range to valid ticks:");
            println!("Original range: {} - {}", lower_price, upper_price);
            println!("Aligned range: {} - {}", actual_lower_price, actual_upper_price);
            println!("Tick indices: {} - {}", tick_lower_index, tick_upper_index);
            
            // Get balances
            let token_a_balance = self.get_balance(self.token_mint_a).await?;
            let token_b_balance = self.get_balance(self.token_mint_b).await?;
            println!("Token A balance ({:?}): {} lamports", self.token_mint_a, token_a_balance);
            println!("Token B balance ({:?}): {} lamports", self.token_mint_b, token_b_balance);

            // Check if we need to swap tokens based on which token is SOL
            let is_token_a_sol = self.token_mint_a == sol_mint;
            let is_token_b_sol = self.token_mint_b == sol_mint;
            
            // Make sure token accounts exist
            if is_token_a_sol {
                // Ensure token B account exists
                println!("Checking if token B account exists...");
                self.create_token_account_if_needed(self.token_mint_b).await?;
            } else if is_token_b_sol {
                // Ensure token A account exists
                println!("Checking if token A account exists...");
                self.create_token_account_if_needed(self.token_mint_a).await?;
            } else {
                // Both token accounts must exist
                self.create_token_account_if_needed(self.token_mint_a).await?;
                self.create_token_account_if_needed(self.token_mint_b).await?;
            }
            
            // Re-check balances after creating token accounts
            let token_a_balance = self.get_balance(self.token_mint_a).await?;
            let token_b_balance = self.get_balance(self.token_mint_b).await?;
            println!("Token A balance after account creation: {} lamports", token_a_balance);
            println!("Token B balance after account creation: {} lamports", token_b_balance);
            
            // Use a small test amount for quotation to prevent early failures
            // This is just to estimate the ratio, not for actual execution
            let test_amount_a = 1_000_000; // Use a small amount for quotes (0.001 SOL)
            let test_amount_b = 1_000;     // Small USDC amount
            
            // Calculate token requirements based on current price and position range
            if is_token_a_sol {
                // SOL is token A
                println!("Getting position requirements quote from SDK...");
                
                // Try to get quote with small test amount instead of real amount
                // This helps avoid "insufficient balance" errors in the quoting phase
                let test_param = IncreaseLiquidityParam::TokenA(test_amount_a);
                
                let quote_result = match open_position_instructions(
                    &self.client.rpc,
                    self.pool_address,
                    actual_lower_price,
                    actual_upper_price,
                    test_param,
                    Some(100), // 1% slippage
                    Some(self.wallet.pubkey()),
                ).await {
                    Ok(result) => result,
                    Err(e) => {
                        println!("Warning: Initial quote failed with test amount: {}", e);
                        println!("Continuing with token balance and swap estimation based on price...");
                        
                        // Fallback to price-based estimation
                        let approx_usdc_needed = (investment_amount as f64 * 0.5 / 1_000_000_000.0 * current_price) as u64;
                        println!("Based on current price, need approximately {} USDC", approx_usdc_needed);
                        
                        if token_b_balance < approx_usdc_needed && token_a_balance >= investment_amount / 2 {
                            // Swap approximately half the amount of SOL to USDC
                            let sol_to_swap = investment_amount / 2;
                            println!("Attempting to swap {} SOL for USDC", sol_to_swap as f64 / 1_000_000_000.0);
                            self.swap_tokens(sol_to_swap, self.token_mint_a, self.token_mint_b).await?;
                            
                            // Wait for swap to complete and balances to update
                            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                        }
                        
                        // Now try opening the position directly
                        println!("Attempting to open position with available balances");
                        self.open_position(actual_lower_price, actual_upper_price, investment_amount).await?;
                        return Ok(());
                    }
                };
                
                // Calculate required token amounts based on the ratio from the test quote
                let test_ratio_a = quote_result.quote.token_max_a as f64 / test_amount_a as f64;
                let test_ratio_b = quote_result.quote.token_max_b as f64 / test_amount_a as f64;
                
                // UPDATED APPROACH: Distribute the investment across both tokens
                // Calculate what proportion of the total investment should go to each token
                let total_test_value = test_amount_a as f64 + (test_amount_a as f64 * test_ratio_b / test_ratio_a);
                let token_a_proportion = test_amount_a as f64 / total_test_value;
                
                // Allocate investment amount proportionally
                let investment_for_token_a = (investment_amount as f64 * token_a_proportion) as u64;
                
                // Calculate required amounts based on this proportion
                let required_a = (investment_for_token_a as f64 * test_ratio_a) as u64;
                let required_b = (investment_for_token_a as f64 * test_ratio_b) as u64;
                
                println!("Estimated token requirements (distributed investment):");
                println!("Required SOL (token A): {} lamports ({} SOL)", 
                    required_a, required_a as f64 / 1_000_000_000.0);
                println!("Required USDC (token B): {} units ({} USDC)", 
                    required_b, required_b as f64 / 1_000_000.0);
                
                // Need SOL (token A) for the position only - gas already accounted for
                let sol_needed = required_a;
                println!("SOL needed for position: {} lamports ({} SOL)", 
                    sol_needed, sol_needed as f64 / 1_000_000_000.0);
                println!("Total SOL needed with gas: {} lamports ({} SOL)",
                    sol_needed + gas_reserve, (sol_needed + gas_reserve) as f64 / 1_000_000_000.0);
                
                // Check if we have enough SOL for position and gas
                if token_a_balance < (sol_needed + gas_reserve) {
                    println!("Not enough SOL. Have: {}, Need: {}", token_a_balance, sol_needed + gas_reserve);
                    
                    // Calculate SOL value of USDC at current price (considering decimal differences)
                    let decimals_factor = 10u64.pow((self.token_mint_a_decimals - self.token_mint_b_decimals) as u32);
                    let usdc_in_sol_value = (token_b_balance as f64 * decimals_factor as f64 / current_price) as u64;
                    let total_value_in_sol = token_a_balance + usdc_in_sol_value;
                    
                    println!("USDC balance in SOL terms: {} lamports ({} SOL)", 
                        usdc_in_sol_value, usdc_in_sol_value as f64 / 1_000_000_000.0);
                    println!("Total value in SOL terms: {} lamports ({} SOL)", 
                        total_value_in_sol, total_value_in_sol as f64 / 1_000_000_000.0);
                    
                    // If total value is enough but SOL balance is low, try to reduce position size
                    if total_value_in_sol >= (sol_needed + gas_reserve) {
                        // We have enough total value, just distributed across tokens
                        println!("You have enough total value, just not enough SOL. Adjusting approach...");
                        
                        // Try smaller position size
                        if retry_count < max_retries {
                            let adjusted_amount = (current_amount as f64 * 0.95) as u64; // 5% smaller
                            if adjusted_amount < 50_000_000 { // If position gets too small (< 0.05 SOL)
                                return Err(anyhow::anyhow!("Position would be too small after adjustment. Consider consolidating your funds into SOL."));
                            }
                            
                            println!("Retrying with adjusted investment amount: {} SOL", 
                                adjusted_amount as f64 / 1_000_000_000.0);
                            
                            // Update the amount and continue to the next iteration
                            current_amount = adjusted_amount;
                            retry_count += 1;
                            continue;
                        } else {
                            return Err(anyhow::anyhow!("Failed to adjust position size after {} attempts. Consider consolidating your funds into SOL.", max_retries));
                        }
                    }
                    
                    return Err(anyhow::anyhow!("Insufficient SOL balance. Have: {} SOL, Need: {} SOL", 
                        token_a_balance as f64 / 1_000_000_000.0, sol_needed as f64 / 1_000_000_000.0));
                }
                
                // Check if we have enough token B
                if token_b_balance < required_b {
                    println!("Not enough token B (USDC). Have: {}, Need: {}. Will swap SOL for USDC.", 
                        token_b_balance, required_b);
                    
                    // Calculate how much SOL to swap to get the needed token B
                    // Add 2% extra to account for slippage and fees
                    let token_b_shortfall = required_b - token_b_balance;
                    let token_b_shortfall_with_buffer = (token_b_shortfall as f64 * 1.02) as u64;
                    
                    // Convert USDC amount to SOL based on current price
                    // Note: We need to account for decimal differences
                    let decimals_factor = 10u64.pow((self.token_mint_a_decimals - self.token_mint_b_decimals) as u32) as f64;
                    let mut sol_to_swap = (token_b_shortfall_with_buffer as f64 * decimals_factor * current_price) as u64;
                    
                    println!("Swapping {} SOL ({} lamports) for {} USDC", 
                        sol_to_swap as f64 / 1_000_000_000.0, sol_to_swap, token_b_shortfall_with_buffer as f64 / 1_000_000.0);
                    
                    // Check if we have enough SOL to swap and still meet our needs
                    if token_a_balance - sol_to_swap < (sol_needed + gas_reserve) {
                        println!("Not enough SOL to both swap and maintain position. Adjusting swap amount.");
                        // Calculate maximum SOL available for swap
                        let max_swap_available = token_a_balance - (sol_needed + gas_reserve);
                        if max_swap_available > 1_000_000 { // Only if we have at least 0.001 SOL available
                            sol_to_swap = max_swap_available;
                            println!("Adjusted swap amount to {} SOL", sol_to_swap as f64 / 1_000_000_000.0);
                        } else {
                            println!("Insufficient SOL to swap and maintain position");
                            return Err(anyhow::anyhow!("Insufficient SOL to swap for USDC and maintain position"));
                        }
                    }
                    
                    // Swap SOL for token B
                    self.swap_tokens(sol_to_swap, self.token_mint_a, self.token_mint_b).await?;
                    
                    // Wait a moment for balances to update and verify
                    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                    let new_token_b_balance = self.get_balance(self.token_mint_b).await?;
                    println!("Updated USDC balance: {} (needed: {})", new_token_b_balance, required_b);
                    
                    if new_token_b_balance < required_b {
                        println!("Warning: After swap, still don't have enough USDC. Have: {}, Need: {}", 
                            new_token_b_balance, required_b);
                        println!("Will attempt to open position with available balances anyway.");
                    }
                }
            } else if is_token_b_sol {
                // SOL is token B - Similar logic as above but for the other direction
                println!("Getting position requirements quote from SDK...");
                
                // Try to get quote with small test amount
                let test_param = IncreaseLiquidityParam::TokenB(test_amount_b);
                
                let quote_result = match open_position_instructions(
                    &self.client.rpc,
                    self.pool_address,
                    actual_lower_price,
                    actual_upper_price,
                    test_param,
                    Some(100), // 1% slippage
                    Some(self.wallet.pubkey()),
                ).await {
                    Ok(result) => result,
                    Err(e) => {
                        println!("Warning: Initial quote failed with test amount: {}", e);
                        println!("Continuing with token balance and swap estimation based on price...");
                        
                        // Fallback to price-based estimation
                        let approx_token_a_needed = (investment_amount as f64 * 0.5 / 1_000_000_000.0 / current_price * 1_000_000.0) as u64;
                        println!("Based on current price, need approximately {} token A", approx_token_a_needed);
                        
                        if token_a_balance < approx_token_a_needed && token_b_balance >= investment_amount / 2 {
                            // Swap approximately half the amount of SOL to token A
                            let sol_to_swap = investment_amount / 2;
                            println!("Attempting to swap {} SOL for token A", sol_to_swap as f64 / 1_000_000_000.0);
                            self.swap_tokens(sol_to_swap, self.token_mint_b, self.token_mint_a).await?;
                            
                            // Wait for swap to complete and balances to update
                            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                        }
                        
                        // Now try opening the position directly
                        println!("Attempting to open position with available balances");
                        self.open_position(actual_lower_price, actual_upper_price, investment_amount).await?;
                        return Ok(());
                    }
                };
                
                // Calculate required token amounts based on the ratio from the test quote
                let test_ratio_a = quote_result.quote.token_max_a as f64 / test_amount_b as f64;
                let test_ratio_b = quote_result.quote.token_max_b as f64 / test_amount_b as f64;
                
                // UPDATED APPROACH: Distribute the investment across both tokens
                // Calculate what proportion of the total investment should go to each token
                let total_test_value = (test_amount_b as f64 * test_ratio_a / test_ratio_b) + test_amount_b as f64;
                let token_b_proportion = test_amount_b as f64 / total_test_value;
                
                // Allocate investment amount proportionally
                let investment_for_token_b = (investment_amount as f64 * token_b_proportion) as u64;
                
                // Calculate required amounts based on this proportion
                let required_a = (investment_for_token_b as f64 * test_ratio_a) as u64;
                let required_b = (investment_for_token_b as f64 * test_ratio_b) as u64;
                
                println!("Estimated token requirements (distributed investment):");
                println!("Required Token A: {} units", required_a);
                println!("Required SOL (token B): {} lamports ({} SOL)", 
                    required_b, required_b as f64 / 1_000_000_000.0);
                
                // Need SOL (token B) for the position only - gas already accounted for
                let sol_needed = required_b;
                println!("SOL needed for position: {} lamports ({} SOL)", 
                    sol_needed, sol_needed as f64 / 1_000_000_000.0);
                println!("Total SOL needed with gas: {} lamports ({} SOL)",
                    sol_needed + gas_reserve, (sol_needed + gas_reserve) as f64 / 1_000_000_000.0);
                
                // Check if we have enough SOL
                if token_b_balance < (sol_needed + gas_reserve) {
                    println!("Not enough SOL. Have: {}, Need: {}", token_b_balance, sol_needed + gas_reserve);
                    
                    // If we don't have enough SOL after multiple retries, return an error
                    if retry_count >= max_retries {
                        return Err(anyhow::anyhow!("Insufficient SOL balance. Have: {} SOL, Need: {} SOL", 
                            token_b_balance as f64 / 1_000_000_000.0, (sol_needed + gas_reserve) as f64 / 1_000_000_000.0));
                    }
                    
                    // Try with a smaller position size
                    let adjusted_amount = (current_amount as f64 * 0.95) as u64; // 5% smaller
                    if adjusted_amount < 50_000_000 { // If position gets too small (< 0.05 SOL)
                        return Err(anyhow::anyhow!("Position would be too small after adjustment. Consider using a larger investment amount."));
                    }
                    
                    println!("Retrying with adjusted investment amount: {} SOL", 
                        adjusted_amount as f64 / 1_000_000_000.0);
                    
                    // Update the amount and continue to the next iteration
                    current_amount = adjusted_amount;
                    retry_count += 1;
                    continue;
                }
                
                // Check if we have enough token A
                if token_a_balance < required_a {
                    println!("Not enough token A. Have: {}, Need: {}. Will swap SOL for token A.", 
                        token_a_balance, required_a);
                    
                    // Calculate how much SOL to swap to get the needed token A
                    // Add 2% extra to account for slippage and fees
                    let token_a_shortfall = required_a - token_a_balance;
                    let token_a_shortfall_with_buffer = (token_a_shortfall as f64 * 1.02) as u64;
                    
                    // Convert token A amount to SOL based on current price
                    // Note: We need to account for decimal differences
                    let decimals_factor = 10u64.pow((self.token_mint_b_decimals - self.token_mint_a_decimals) as u32) as f64;
                    let mut sol_to_swap = (token_a_shortfall_with_buffer as f64 * current_price * decimals_factor) as u64;
                    
                    println!("Swapping {} SOL ({} lamports) for {} token A", 
                        sol_to_swap as f64 / 1_000_000_000.0, sol_to_swap, token_a_shortfall_with_buffer);
                    
                    // Check if we have enough SOL to swap and still meet our needs
                    if token_b_balance - sol_to_swap < (sol_needed + gas_reserve) {
                        println!("Not enough SOL to both swap and maintain position. Adjusting swap amount.");
                        // Calculate maximum SOL available for swap
                        let max_swap_available = token_b_balance - (sol_needed + gas_reserve);
                        if max_swap_available > 1_000_000 { // Only if we have at least 0.001 SOL available
                            sol_to_swap = max_swap_available;
                            println!("Adjusted swap amount to {} SOL", sol_to_swap as f64 / 1_000_000_000.0);
                        } else {
                            println!("Insufficient SOL to swap and maintain position");
                            return Err(anyhow::anyhow!("Insufficient SOL to swap for token A and maintain position"));
                        }
                    }
                    
                    // Swap SOL for token A
                    self.swap_tokens(sol_to_swap, self.token_mint_b, self.token_mint_a).await?;
                    
                    // Wait a moment for balances to update and verify
                    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                    let new_token_a_balance = self.get_balance(self.token_mint_a).await?;
                    println!("Updated token A balance: {} (needed: {})", new_token_a_balance, required_a);
                    
                    if new_token_a_balance < required_a {
                        println!("Warning: After swap, still don't have enough token A. Have: {}, Need: {}", 
                            new_token_a_balance, required_a);
                        println!("Will attempt to open position with available balances anyway.");
                    }
                }
            } else {
                // Neither token is SOL - not supported yet
                return Err(anyhow::anyhow!("Neither token is SOL. Currently only supporting pools with SOL as one of the tokens."));
            }
            
            // Now open the position with the aligned prices
            println!("Opening position with aligned range {}–{}", actual_lower_price, actual_upper_price);
            self.open_position(actual_lower_price, actual_upper_price, investment_amount).await?;
            return Ok(());
        }
    }

    pub async fn open_position(&mut self, lower_price: f64, upper_price: f64, amount: u64) -> Result<()> {
        let sol_mint = Pubkey::from_str("So11111111111111111111111111111111111111112")?;
        
        // Use explicit liquidity parameter instead of token amount
        // This will ensure the SDK calculates the optimal token distribution
        let param = if self.token_mint_a == sol_mint {
            // If token A is SOL
            IncreaseLiquidityParam::TokenA(amount / 2)
        } else if self.token_mint_b == sol_mint {
            // If token B is SOL
            IncreaseLiquidityParam::TokenB(amount / 2)
        } else {
            // Fallback for non-SOL pairs (not currently handling)
            IncreaseLiquidityParam::TokenA(amount)
        };

        println!("Creating position with price range: {} - {}", lower_price, upper_price);
        let result = open_position_instructions(
            &self.client.rpc,
            self.pool_address,
            lower_price,
            upper_price,
            param,
            Some(100), // 1% slippage
            Some(self.wallet.pubkey()),
        ).await.map_err(|e| anyhow::anyhow!("Open position instruction error: {}", e))?;

        println!("Position creation details:");
        println!("Token max A: {}", result.quote.token_max_a);
        println!("Token max B: {}", result.quote.token_max_b);
        println!("Liquidity: {}", result.quote.liquidity_delta);

        // Prepare all signers including any additional ones provided by the SDK
        let mut signers: Vec<&dyn Signer> = vec![self.wallet.as_ref()];
        
        // Add any additional signers from the SDK result
        if !result.additional_signers.is_empty() {
            println!("Adding {} additional signers from SDK", result.additional_signers.len());
            signers.extend(
                result.additional_signers.iter()
                    .map(|kp| kp as &dyn Signer),
            );
        }
        
        let recent_blockhash = self.client.rpc.get_latest_blockhash().await?;
        
        // Create transaction with all required signers
        let tx = Transaction::new_signed_with_payer(
            &result.instructions,
            Some(&self.wallet.pubkey()),
            &signers,
            recent_blockhash,
        );
        
        println!("Submitting transaction to open position with {} signers...", signers.len());
        let signature = match self.client.send_and_confirm_transaction_with_commitment(&tx, CommitmentLevel::Confirmed).await {
            Ok(sig) => sig,
            Err(e) => {
                println!("Transaction failed: {}", e);
                println!("This could be due to insufficient token balances or price movement.");
                println!("Try checking your balances and the current price of the pool.");
                return Err(anyhow::anyhow!("Failed to open position: {}", e));
            }
        };
        
        println!("Opened position successfully. Signature: {}", signature);

        let (position_address, _) = get_position_address(&result.position_mint)?;
        let whirlpool_account = self.client.rpc.get_account(&self.pool_address).await?;
        let whirlpool = Whirlpool::from_bytes(&whirlpool_account.data)
            .map_err(|e| anyhow::anyhow!("Failed to deserialize Whirlpool: {}", e))?;
            
        // Store correct tick indices for this position
        let tick_spacing = whirlpool.tick_spacing as i32; // Convert to i32 for calculations
        
        // Convert price to tick index
        let price_to_tick_index = |price: f64| -> i32 {
            let raw_tick_index = (price.ln() / 1.0001f64.ln()) as i32;
            // Round to nearest valid tick based on spacing
            (raw_tick_index / tick_spacing) * tick_spacing
        };
        
        let tick_lower_index = price_to_tick_index(lower_price);
        let tick_upper_index = price_to_tick_index(upper_price);

        self.position = Some(Position {
            lower_price,
            upper_price,
            mint_address: result.position_mint,
            address: position_address,
            tick_lower_index,
            tick_upper_index,
            liquidity: result.quote.liquidity_delta,
        });
        
        // Add a small delay to give time for the position accounts to be fully created and propagated
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
        
        self.display_balances().await?;
        Ok(())
    }

    pub async fn close_position(&mut self) -> Result<(Signature, u128)> {
        if let Some(ref position) = self.position {
            println!("Closing position: {}", position.mint_address);

            let result = close_position_instructions(
                &self.client.rpc,
                position.mint_address,
                Some(100),
                Some(self.wallet.pubkey()),
            ).await.map_err(|e| anyhow::anyhow!("Close position instruction error: {}", e))?;

            // Prepare all signers including any additional ones provided by the SDK
            let mut signers: Vec<&dyn Signer> = vec![self.wallet.as_ref()];
            
            // Add any additional signers from the SDK result
            if !result.additional_signers.is_empty() {
                println!("Adding {} additional signers from SDK", result.additional_signers.len());
                signers.extend(
                    result.additional_signers.iter()
                        .map(|kp| kp as &dyn Signer),
                );
            }

            let recent_blockhash = self.client.rpc.get_latest_blockhash().await?;
            let tx = Transaction::new_signed_with_payer(
                &result.instructions,
                Some(&self.wallet.pubkey()),
                &signers,
                recent_blockhash,
            );
            let signature = self.client.send_and_confirm_transaction_with_commitment(&tx, CommitmentLevel::Confirmed).await?;
            println!("Closed position. Signature: {}", signature);

            let liquidity = result.quote.liquidity_delta;
            self.position = None;
            self.display_balances().await?;
            Ok((signature, liquidity))
        } else {
            Err(anyhow::anyhow!("No position to close"))
        }
    }

    pub async fn get_position(&self) -> Result<Position> {
        self.position.clone().ok_or_else(|| anyhow::anyhow!("No active position"))
    }

    async fn display_balances(&self) -> Result<()> {
        let token_a_balance = self.get_balance(self.token_mint_a).await?;
        let token_b_balance = self.get_balance(self.token_mint_b).await?;
        println!(
            "Wallet Balances:\n- Token A ({:?}): {} lamports\n- Token B ({:?}): {} lamports",
            self.token_mint_a, token_a_balance, self.token_mint_b, token_b_balance
        );

        if let Some(ref position) = self.position {
            // Try to get position balances, but don't fail the whole method if it doesn't work
            match close_position_instructions(
                &self.client.rpc,
                position.mint_address,
                Some(100),
                Some(self.wallet.pubkey()),
            ).await {
                Ok(close_result) => {
                    let token_a_balance = close_result.quote.token_est_a as f64 / 10f64.powi(self.token_mint_a_decimals as i32);
                    let token_b_balance = close_result.quote.token_est_b as f64 / 10f64.powi(self.token_mint_b_decimals as i32);
                    println!(
                        "Position Balances:\n- Token A ({:?}): {:.6}\n- Token B ({:?}): {:.6}",
                        self.token_mint_a, token_a_balance, self.token_mint_b, token_b_balance
                    );
                },
                Err(e) => {
                    println!("Note: Could not retrieve position balances (this is normal for newly created positions): {}", e);
                    println!("Position details: Mint: {}, Price range: {}-{}", 
                        position.mint_address, position.lower_price, position.upper_price);
                }
            }
        }
        Ok(())
    }

    pub async fn load_position(&mut self, position_mint_address: &str) -> Result<()> {
        if position_mint_address.is_empty() {
            return Ok(());
        }

        let position_mint = Pubkey::from_str(position_mint_address)?;
        let (position_address, _) = get_position_address(&position_mint)?;
        
        // Prefix with underscore to indicate intentional non-use
        let _position_account = self.client.rpc.get_account(&position_address).await?;
        
        let whirlpool = self.get_whirlpool().await?;
        
        self.position = Some(Position {
            lower_price: 0.0, // These would be calculated from actual position data
            upper_price: 0.0,
            mint_address: position_mint,
            address: position_address,
            tick_lower_index: whirlpool.tick_current_index - 100, // Placeholder
            tick_upper_index: whirlpool.tick_current_index + 100, // Placeholder
            liquidity: 0, // Would be fetched from actual position data
        });
        
        println!("Loaded existing position: {}", position_mint);
        Ok(())
    }

    pub async fn get_native_sol_balance(&self) -> Result<u64> {
        match self.client.rpc.get_balance(&self.wallet.pubkey()).await {
            Ok(balance) => {
                println!("Native SOL balance: {} lamports ({:.6} SOL)", 
                    balance, balance as f64 / 1_000_000_000.0);
                Ok(balance)
            },
            Err(e) => {
                println!("Error fetching native SOL balance: {}", e);
                Err(anyhow::anyhow!("Failed to get native SOL balance: {}", e))
            }
        }
    }
}