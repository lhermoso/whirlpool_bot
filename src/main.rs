use std::env;
use std::str::FromStr; // Added missing import
use std::time::Duration;
use solana_sdk::pubkey::Pubkey;
use tokio;

mod cli;
mod position_manager;
mod solana_utils;
mod wallet;

use cli::CliArgs;
use position_manager::PositionManager;
use solana_utils::SolanaRpcClient;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenv::dotenv().ok();
    env_logger::init();

    let args = cli::parse_args();
    let rpc_url = env::var("RPC_URL").expect("RPC_URL must be set in .env");
    let client = SolanaRpcClient::new(&rpc_url)?;

    let wallet = wallet::load_wallet();
    let pool_address = Pubkey::from_str(&args.pool_address)
        .map_err(|e| anyhow::anyhow!("Invalid pool address: {}", e))?;
    let mut position_manager = PositionManager::new(&client, wallet, pool_address).await?;

    let initial_mid_price = 150.0;
    let range_width = 20.0;
    let lower_price = initial_mid_price - (range_width / 2.0);
    let upper_price = initial_mid_price + (range_width / 2.0);
    let amount = (args.invest * 1_000_000_000.0) as u64;

    let position_mint = Pubkey::from_str(&args.position_mint_address).ok();
    if position_mint.is_none() && !position_manager.has_existing_position().await? {
        println!("No position found. Opening initial position at {}–{}", lower_price, upper_price);
        position_manager.open_position_with_balance_check(lower_price, upper_price, amount).await?;
    }

    loop {
        let current_price = position_manager.get_current_price().await?;
        println!("Current price: {}", current_price);

        let position = position_manager.get_position().await?;
        let whirlpool = position_manager.get_whirlpool().await?;
        let in_range = orca_whirlpools_core::is_position_in_range(
            whirlpool.sqrt_price.into(),
            position.tick_lower_index,
            position.tick_upper_index,
        );

        if !in_range {
            println!("Price {} outside range {}–{}. Rebalancing...", current_price, position.lower_price, position.upper_price);
            position_manager.rebalance(current_price, amount).await?;

            if current_price < position.lower_price {
                let new_lower = current_price - (range_width / 2.0);
                let new_upper = current_price + (range_width / 2.0);
                println!("Opening new position at {}–{}", new_lower, new_upper);
                position_manager.open_position_with_balance_check(new_lower, new_upper, amount).await?;
            }
        } else {
            println!("Price within range {}–{}", position.lower_price, position.upper_price);
        }

        tokio::time::sleep(Duration::from_secs(args.interval)).await;
    }
}