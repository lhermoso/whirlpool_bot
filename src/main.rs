use std::env;
use std::str::FromStr;
use std::time::Duration;
use solana_sdk::pubkey::Pubkey;
use tokio;
use orca_whirlpools::set_whirlpools_config_address;
use orca_whirlpools::WhirlpoolsConfigInput;
use clap::Parser;
use tokio::time::Duration;
use anyhow::Result;

mod cli;
mod position_manager;
mod solana_utils;
mod wallet;

use position_manager::PositionManager;
use solana_utils::SolanaRpcClient;

// Helper function to calculate optimal investment amount based on portfolio value
async fn calculate_optimal_investment_amount(
    position_manager: &PositionManager,
    target_amount: u64,
    min_viable_position: u64
) -> Result<Option<u64>> {
    // Get total portfolio value considering both SOL and USDC
    let (sol_balance, usdc_balance, total_value_in_sol) = position_manager.get_total_portfolio_value().await?;
    
    // Decide on investment amount based on total portfolio value
    let invest_amount = if total_value_in_sol < target_amount {
        println!("Warning: Total portfolio value {} SOL is less than requested investment amount {} SOL.", 
            total_value_in_sol as f64 / 1_000_000_000.0, target_amount as f64 / 1_000_000_000.0);
        println!("Using available balance with reserve for gas.");
        total_value_in_sol.saturating_sub(10_000_000) // Reserve 0.01 SOL for gas
    } else if sol_balance < target_amount && usdc_balance > 0 {
        // We have enough total value, but it's distributed across tokens
        println!("We have enough total value ({} SOL), but need to rebalance between SOL and USDC.", 
            total_value_in_sol as f64 / 1_000_000_000.0);
        
        // Try to swap USDC to SOL to meet the target investment amount
        let sol_needed = target_amount.saturating_sub(sol_balance);
        println!("Need additional {} SOL. Will try to swap from USDC.", 
            sol_needed as f64 / 1_000_000_000.0);
        
        // Attempt to swap USDC to SOL
        let sol_mint = Pubkey::from_str("So11111111111111111111111111111111111111112")?;
        match position_manager.swap_usdc_to_sol_if_needed(target_amount, 10_000_000).await {
            Ok(true) => {
                // Successfully swapped enough USDC to SOL
                println!("Successfully converted USDC to SOL.");
                let updated_sol_balance = position_manager.get_balance(sol_mint).await?;
                if updated_sol_balance < target_amount {
                    updated_sol_balance.saturating_sub(10_000_000) // Still reserve some gas
                } else {
                    target_amount
                }
            },
            _ => {
                // Couldn't swap enough, use what we have
                println!("Could not swap enough USDC to SOL. Using available SOL balance.");
                sol_balance.saturating_sub(10_000_000)
            }
        }
    } else {
        // We have enough SOL directly
        target_amount
    };
    
    // Ensure minimum viable position size
    if invest_amount < min_viable_position {
        println!("Warning: Available investment amount {} SOL is below minimum viable position size {} SOL", 
            invest_amount as f64 / 1_000_000_000.0, min_viable_position as f64 / 1_000_000_000.0);
        println!("Skipping position creation until more funds are available");
        return Ok(None);
    }
    
    Ok(Some(invest_amount))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenv::dotenv().ok();
    env_logger::init();
    
    let args = cli::parse_args();

    // Print the custom start screen
    println!(
        "\n\
        =============================\n\
        🌀 Le∞ Hermos🌀 Whirlpool Bot \n\
        =============================\n"
    );
    println!("Configuration:");
    println!(
        "  Network: {}\n\
          Invest Amount: {} SOL\n\
          Range Percentage: {:.2}%\n\
          Interval: {} seconds\n\
          Pool Address: {}\n\
          Position Mint Address: {}",
        args.network,
        args.invest,
        args.range_percentage,
        args.interval,
        args.pool_address,
        if args.position_mint_address.is_empty() { "Auto-detect" } else { &args.position_mint_address }
    );
    println!("-------------------------------------\n");

    
    // Set the Whirlpools config based on the network parameter
    match args.network.to_lowercase().as_str() {
        "devnet" => {
            println!("Using Solana Devnet");
            set_whirlpools_config_address(WhirlpoolsConfigInput::SolanaDevnet).unwrap();
        },
        "mainnet" => {
            println!("Using Solana Mainnet");
            set_whirlpools_config_address(WhirlpoolsConfigInput::SolanaMainnet).unwrap();
        },
        _ => {
            return Err(anyhow::anyhow!("Invalid network: {}. Must be 'mainnet' or 'devnet'", args.network));
        }
    }

    
    // Get the appropriate RPC URL based on the network
    let rpc_url = match args.network.to_lowercase().as_str() {
        "devnet" => env::var("RPC_DEV_URL").expect("RPC_DEV_URL must be set in .env for devnet"),
        "mainnet" => env::var("RPC_URL").expect("RPC_URL must be set in .env for mainnet"),
        _ => unreachable!(), // We already validated the network above
    };
    
    let client = SolanaRpcClient::new(&rpc_url)?;
   
    // Validate the pool address format
    println!("Parsing pool address: {}", args.pool_address);
    let pool_address = match Pubkey::from_str(&args.pool_address) {
        Ok(pubkey) => pubkey,
        Err(e) => {
            println!("Failed to parse pool address: {}", e);
            return Err(anyhow::anyhow!("Invalid pool address format: {}. Must be a valid Solana public key (44 characters, base58 encoded)", args.pool_address));
        }
    };
    println!("Pool address parsed successfully: {}", pool_address);
    
    // Verify the account exists before initializing position manager
    println!("Verifying pool account exists...");
    match client.rpc.get_account(&pool_address).await {
        Ok(_) => println!("Pool account exists!"),
        Err(e) => {
            println!("Failed to fetch pool account: {}", e);
            return Err(anyhow::anyhow!("Pool account not found or network error. Make sure the pool address is correct and you're connected to the right network. Error: {}", e));
        }
    }

    let wallet = wallet::load_wallet();
    println!("Wallet loaded successfully: {}", wallet.pubkey());
    
    // Try to initialize position manager with better error reporting
    println!("Initializing position manager...");
    let mut position_manager = match PositionManager::new(client, wallet, pool_address).await {
        Ok(pm) => {
            println!("Position manager initialized successfully!");
            pm
        },
        Err(e) => {
            println!("Failed to initialize position manager: {}", e);
            println!("This could be because:");
            println!("- The pool address is not a valid Whirlpool pool");
            println!("- There's an issue with the RPC connection");
            println!("- The Whirlpool account data is corrupted or in an unexpected format");
            return Err(anyhow::anyhow!("Failed to initialize position manager: {}", e));
        }
    };

    // Check native SOL balance explicitly
    let sol_balance = position_manager.get_native_sol_balance().await?;
    println!("Native SOL balance: {} lamports ({:.6} SOL)", 
        sol_balance, sol_balance as f64 / 1_000_000_000.0);
    
    if sol_balance < (args.invest * 1_000_000_000.0) as u64 + 50_000_000 {
        println!("Warning: Your SOL balance may be insufficient for the requested investment amount");
        println!("You have: {:.6} SOL, Want to invest: {:.6} SOL (plus 0.05 SOL gas reserve)", 
            sol_balance as f64 / 1_000_000_000.0, args.invest);
        println!("Consider using a smaller --invest amount");
    }
 
    // First try to load any existing positions for the wallet
    let position_loaded = position_manager.load_positions_for_wallet().await?;
    
    // If no positions found for wallet and a specific position mint address is provided, try loading that
    if !position_loaded && !args.position_mint_address.is_empty() {
        println!("No positions found for wallet, trying to load specific position...");
        position_manager.load_position(&args.position_mint_address).await?;
    }

    let amount = (args.invest * 1_000_000_000.0) as u64;
    let range_percentage = args.range_percentage;

    // Main monitoring loop
    loop {
        let current_price = position_manager.get_current_price().await?;
        println!("Current price: {}", current_price);

        // Check if position exists and is in range
        match position_manager.get_position().await {
            Ok(position) => {
                let whirlpool = position_manager.get_whirlpool().await?;
                let in_range = orca_whirlpools_core::is_position_in_range(
                    whirlpool.sqrt_price.into(),
                    position.tick_lower_index,
                    position.tick_upper_index,
                );

                // Rebalance if out of range
                if !in_range {
                    println!("Price {} outside range {}–{}. Rebalancing...", 
                        current_price, position.lower_price, position.upper_price);
                    
                    // Calculate new range centered on current price using percentage
                    let half_percentage = range_percentage;
                    let new_lower = current_price * (1.0 - half_percentage / 100.0);
                    let new_upper = current_price * (1.0 + half_percentage / 100.0);
                    
                    // Multiple attempts to close the position, with backoff
                    let mut close_attempts = 0;
                    let max_close_attempts = 3;
                    let mut close_successful = false;
                    
                    while close_attempts < max_close_attempts && !close_successful {
                        if close_attempts > 0 {
                            println!("Retrying position closure (attempt {}/{})", close_attempts + 1, max_close_attempts);
                            // Wait with increasing backoff before retrying
                            tokio::time::sleep(Duration::from_secs(5 * close_attempts as u64)).await;
                        }
                        
                        // Attempt to close existing position
                        match position_manager.close_position().await {
                            Ok(result) => {
                                println!("Successfully closed position with signature: {}", result.0);
                                close_successful = true;
                                
                                println!("Opening new position at {}–{} (±{}%)", new_lower, new_upper, half_percentage);
                                
                                // Calculate optimal investment amount considering both SOL and USDC
                                let min_viable_position = 50_000_000; // 0.05 SOL
                                match calculate_optimal_investment_amount(&position_manager, amount, min_viable_position).await? {
                                    Some(invest_amount) => {
                                        // Open position with the optimal amount
                                        println!("Opening new position with {} SOL", invest_amount as f64 / 1_000_000_000.0);
                                        position_manager.open_position_with_balance_check(new_lower, new_upper, invest_amount).await?;
                                    },
                                    None => {
                                        // Not enough funds to open a viable position
                                        println!("Insufficient funds to open a viable position. Waiting for next check.");
                                    }
                                }
                            },
                            Err(e) => {
                                println!("Failed to close position: {}. Attempt {}/{}.", e, close_attempts + 1, max_close_attempts);
                                close_attempts += 1;
                            }
                        }
                    }
                    
                    if !close_successful {
                        println!("Failed to close position after {} attempts. Will retry next interval.", max_close_attempts);
                    }
                } else {
                    println!("Price within range {}–{}", position.lower_price, position.upper_price);
                }
            },
            Err(e) => {
                println!("No active position found: {}. Creating initial position.", e);
                
                // Calculate range based on percentage
                let lower_price = current_price * (1.0 - range_percentage / 100.0);
                let upper_price = current_price * (1.0 + range_percentage / 100.0);
                
                println!("Opening initial position at {}–{} (±{}%)", 
                    lower_price, upper_price, range_percentage);
                
                // Calculate optimal investment amount considering both SOL and USDC
                let min_viable_position = 50_000_000; // 0.05 SOL
                match calculate_optimal_investment_amount(&position_manager, amount, min_viable_position).await? {
                    Some(invest_amount) => {
                        // Open position with the optimal amount
                        println!("Opening initial position with {} SOL", invest_amount as f64 / 1_000_000_000.0);
                        position_manager.open_position_with_balance_check(lower_price, upper_price, invest_amount).await?;
                    },
                    None => {
                        // Not enough funds to open a viable position
                        println!("Insufficient funds to open a viable position. Waiting for next check.");
                    }
                }
            }
        }

        // Wait for next check
        tokio::time::sleep(Duration::from_secs(args.interval)).await;
    }
}