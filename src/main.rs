use std::env;
use std::str::FromStr;
use std::time::Duration;
use solana_sdk::pubkey::Pubkey;
use tokio;
use orca_whirlpools::set_whirlpools_config_address;
use orca_whirlpools::WhirlpoolsConfigInput;

mod cli;
mod position_manager;
mod solana_utils;
mod wallet;

use position_manager::PositionManager;
use solana_utils::SolanaRpcClient;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenv::dotenv().ok();
    env_logger::init();
    
    let args = cli::parse_args();
    
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

    let wallet = wallet::load_wallet();
    let pool_address = Pubkey::from_str(&args.pool_address)
        .map_err(|e| anyhow::anyhow!("Invalid pool address: {}", e))?;
    let mut position_manager = PositionManager::new(client, wallet, pool_address).await?;

    // Try to load existing position if mint address is provided
    if !args.position_mint_address.is_empty() {
        position_manager.load_position(&args.position_mint_address).await?;
    }

    let amount = (args.invest * 1_000_000_000.0) as u64;
    let range_percentage = args.range_percentage;

    // If no position exists, open an initial one
    if position_manager.get_position().await.is_err() {
        let current_price = position_manager.get_current_price().await?;
        
        // Calculate range based on percentage
        let half_percentage = range_percentage / 2.0;
        let lower_price = current_price * (1.0 - half_percentage / 100.0);
        let upper_price = current_price * (1.0 + half_percentage / 100.0);
        
        println!("No position found. Opening initial position at {}–{} (±{}%)", 
            lower_price, upper_price, half_percentage);
        position_manager.open_position_with_balance_check(lower_price, upper_price, amount).await?;
    }

    // Main monitoring loop
    loop {
        let current_price = position_manager.get_current_price().await?;
        println!("Current price: {}", current_price);

        // Check if position is in range
        let position = position_manager.get_position().await?;
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
            let half_percentage = range_percentage / 2.0;
            let new_lower = current_price * (1.0 - half_percentage / 100.0);
            let new_upper = current_price * (1.0 + half_percentage / 100.0);
            
            // Close existing position and open a new one
            position_manager.close_position().await?;
            println!("Opening new position at {}–{} (±{}%)", new_lower, new_upper, half_percentage);
            position_manager.open_position_with_balance_check(new_lower, new_upper, amount).await?;
        } else {
            println!("Price within range {}–{}", position.lower_price, position.upper_price);
        }

        // Wait for next check
        tokio::time::sleep(Duration::from_secs(args.interval)).await;
    }
}