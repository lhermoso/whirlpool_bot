use orca_whirlpools::{
    open_position_instructions, close_position_instructions, swap_instructions,
    set_whirlpools_config_address, IncreaseLiquidityParam, WhirlpoolsConfigInput,
    fetch_positions_for_owner, set_funder // Root import
};
use orca_whirlpools_client::Whirlpool;
use orca_whirlpools_core::{sqrt_price_to_price, tick_index_to_price, tick_index_to_sqrt_price, is_position_in_range};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_client::rpc_request::TokenAccountsFilter;
use solana_sdk::{
    pubkey::Pubkey,
    signature::{Signature, Signer},
    transaction::Transaction,
    program_pack::Pack,
};
use anyhow::{Result, Context};
use spl_token_2022::state::Mint;
use std::collections::HashMap;
use tokio::sync::Mutex;
use std::str::FromStr;

use crate::solana_utils::SolanaRpcClient;

const PROGRAM_ID: Pubkey = Pubkey::new_from_array([
    0x6e, 0xeb, 0x65, 0x4a, 0x8e, 0x36, 0xe7, 0x49, 0xd0, 0x8f, 0xb8, 0x33, 0x5c, 0xd3, 0xa8,
    0xea, 0x6f, 0x73, 0xcc, 0x37, 0x11, 0x6f, 0x2e, 0xc1, 0x9b, 0x9e, 0x99, 0x7e, 0x58, 0x6b,
    0x9b, 0x9c,
]);

fn get_position_address(position_mint: &Pubkey) -> Result<(Pubkey, u8), anyhow::Error> {
    Ok(Pubkey::find_program_address(&[b"position", position_mint.as_ref()], &PROGRAM_ID))
}

pub async fn fetch_mint(rpc: &RpcClient, mint_address: &Pubkey, cache: &Mutex<HashMap<Pubkey, Mint>>) -> Result<Mint> {
    let mut cache_lock = cache.lock().await;
    if let Some(mint) = cache_lock.get(mint_address) {
        return Ok(mint.clone());
    }

    let mint_account = rpc
        .get_account(mint_address)
        .await
        .with_context(|| format!("Failed to fetch account data for mint: {}", mint_address))?;

    let token_program_id = Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKpfXGKxYvhqA")?;
    if mint_account.owner != token_program_id {
        return Err(anyhow::anyhow!(
            "Account {} is not a valid SPL Token mint (owner: {}, expected: {})",
            mint_address,
            mint_account.owner,
            token_program_id
        ));
    }

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
    pub address: Pubkey,
    pub tick_lower_index: i32,
    pub tick_upper_index: i32,
    pub liquidity: u128,
}

pub struct PositionManager {
    pub client: SolanaRpcClient,
    wallet: Box<dyn Signer>,
    pool_address: Pubkey,
    position: Option<Position>,
    rpc: RpcClient,
    token_mint_a: Pubkey,
    token_mint_b: Pubkey,
    token_mint_a_decimals: u8,
    token_mint_b_decimals: u8,
    mint_cache: Mutex<HashMap<Pubkey, Mint>>,
}

impl PositionManager {
    pub async fn new(client: &SolanaRpcClient, wallet: Box<dyn Signer>, pool_address: Pubkey) -> Result<Self> {
        set_whirlpools_config_address(WhirlpoolsConfigInput::SolanaDevnet)
            .map_err(|e| anyhow::anyhow!("Failed to set whirlpools config: {}", e))?;
        set_funder(wallet.pubkey())
            .map_err(|e| anyhow::anyhow!("Failed to set funder: {}", e))?;
        let rpc = RpcClient::new("https://api.devnet.solana.com".to_string());

        let whirlpool_account = client.rpc.get_account(&pool_address).await?;
        let whirlpool = Whirlpool::from_bytes(&whirlpool_account.data)
            .map_err(|e| anyhow::anyhow!("Failed to deserialize Whirlpool: {}", e))?;


        let mint_cache = Mutex::new(HashMap::new());
        let token_mint_a = fetch_mint(&rpc, &whirlpool.token_mint_a, &mint_cache).await?;
        let token_mint_b = fetch_mint(&rpc, &whirlpool.token_mint_b, &mint_cache).await?;

        Ok(PositionManager {
            client: client.clone(),
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
        let whirlpool_account = self.rpc.get_account(&self.pool_address).await?;
        let whirlpool = Whirlpool::from_bytes(&whirlpool_account.data)
            .map_err(|e| anyhow::anyhow!("Failed to deserialize Whirlpool: {}", e))?;
        Ok(sqrt_price_to_price(
            whirlpool.sqrt_price,
            self.token_mint_a_decimals,
            self.token_mint_b_decimals,
        ))
    }

    pub async fn get_whirlpool(&self) -> Result<Whirlpool> {
        let whirlpool_account = self.rpc.get_account(&self.pool_address).await?;
        Whirlpool::from_bytes(&whirlpool_account.data)
            .map_err(|e| anyhow::anyhow!("Failed to deserialize Whirlpool: {}", e))
    }

    pub async fn get_balance(&self, token_mint: Pubkey) -> Result<u64> {
        let token_account = self.rpc.get_token_accounts_by_owner(&self.wallet.pubkey(), TokenAccountsFilter::Mint(token_mint)).await?;
        if let Some(account) = token_account.first() {
            let balance = self.rpc.get_token_account_balance(&account.pubkey()).await?;
            Ok(balance.amount.parse::<u64>()?)
        } else {
            Ok(0)
        }
    }

    pub async fn swap_tokens(&self, amount: u64, from_mint: Pubkey, to_mint: Pubkey) -> Result<Signature> {
        let slippage_tolerance = Some(100u16); // Fixed to Option<u16>
        let swap_result = swap_instructions(
            &self.rpc,
            self.pool_address,
            amount,
            from_mint,
            orca_whirlpools::SwapType::ExactIn,
            slippage_tolerance,
            Some(self.wallet.pubkey()),
        ).await?;

        let recent_blockhash = self.rpc.get_latest_blockhash().await?;
        let tx = Transaction::new_signed_with_payer(
            &swap_result.instructions,
            Some(&self.wallet.pubkey()),
            &[self.wallet.as_ref()],
            recent_blockhash,
        );
        let signature = self.rpc.send_and_confirm_transaction(&tx).await?;
        println!("Swapped {} from {:?} to {:?}. Signature: {}", amount, from_mint, to_mint, signature);
        Ok(signature)
    }

    pub async fn open_position_with_balance_check(&mut self, lower_price: f64, upper_price: f64, amount: u64) -> Result<()> {
        let gas_reserve = 50_000_000;
        let required_amount = amount + gas_reserve;

        let sol_mint = Pubkey::from_str("So11111111111111111111111111111111111111112")?;
        let token_a_balance = self.get_balance(self.token_mint_a).await?;
        let token_b_balance = self.get_balance(self.token_mint_b).await?;
        println!("Token A balance ({:?}): {} lamports", self.token_mint_a, token_a_balance);
        println!("Token B balance ({:?}): {} lamports", self.token_mint_b, token_b_balance);

        let is_token_a_sol = self.token_mint_a == sol_mint;
        let primary_mint = if is_token_a_sol { self.token_mint_a } else { self.token_mint_b };
        let secondary_mint = if is_token_a_sol { self.token_mint_b } else { self.token_mint_a };
        let primary_balance = if is_token_a_sol { token_a_balance } else { token_b_balance };
        let secondary_balance = if is_token_a_sol { token_b_balance } else { token_a_balance };

        if primary_balance >= required_amount {
            println!("Sufficient balance in primary token ({:?}). Opening position...", primary_mint);
            self.open_position(lower_price, upper_price, amount).await?;
        } else {
            println!("Insufficient primary balance. Checking secondary token ({:?})...", secondary_mint);
            let current_price = self.get_current_price().await?;
            let amount_in_usd = amount as f64 * current_price / 1_000_000_000.0;
            let required_secondary = (amount_in_usd * 1_000_000.0) as u64;

            if secondary_balance >= required_secondary {
                println!("Swapping {} from {:?} to {:?}", required_secondary, secondary_mint, primary_mint);
                self.swap_tokens(required_secondary, secondary_mint, primary_mint).await?;
                self.open_position(lower_price, upper_price, amount).await?;
            } else {
                return Err(anyhow::anyhow!("Insufficient balance in secondary token: {} < {}", secondary_balance, required_secondary));
            }
        }
        Ok(())
    }

    pub async fn open_position(&mut self, lower_price: f64, upper_price: f64, amount: u64) -> Result<()> {
        let sol_mint = Pubkey::from_str("So11111111111111111111111111111111111111112")?;
        let param = if self.token_mint_a == sol_mint {
            IncreaseLiquidityParam::TokenA(amount)
        } else {
            IncreaseLiquidityParam::TokenB(amount)
        };

        let result = open_position_instructions(
            &self.rpc,
            self.pool_address,
            lower_price,
            upper_price,
            param,
            Some(100),
            Some(self.wallet.pubkey()),
        ).await?;

        let recent_blockhash = self.rpc.get_latest_blockhash().await?;
        let tx = Transaction::new_signed_with_payer(
            &result.instructions,
            Some(&self.wallet.pubkey()),
            &[self.wallet.as_ref()],
            recent_blockhash,
        );
        let signature = self.rpc.send_and_confirm_transaction(&tx).await?;
        println!("Opened position. Signature: {}", signature);

        let (position_address, _) = get_position_address(&result.position_mint)?;
        let whirlpool_account = self.client.rpc.get_account(&self.pool_address).await?;
        let whirlpool = Whirlpool::from_bytes(&whirlpool_account.data)
            .map_err(|e| anyhow::anyhow!("Failed to deserialize Whirlpool: {}", e))?;
        let tick_lower_index = whirlpool.tick_current_index - 100; // Placeholder
        let tick_upper_index = whirlpool.tick_current_index + 100; // Placeholder

        self.position = Some(Position {
            lower_price,
            upper_price,
            mint_address: result.position_mint,
            address: position_address,
            tick_lower_index,
            tick_upper_index,
            liquidity: result.quote.liquidity_delta,
        });
        self.display_balances().await?;
        Ok(())
    }

    pub async fn close_position(&mut self) -> Result<(Signature, u128)> {
        if let Some(ref position) = self.position {
            println!("Closing position: {}", position.mint_address);

            let result = close_position_instructions(
                &self.rpc,
                position.mint_address,
                Some(100),
                Some(self.wallet.pubkey()),
            ).await?;

            let recent_blockhash = self.rpc.get_latest_blockhash().await?;
            let tx = Transaction::new_signed_with_payer(
                &result.instructions,
                Some(&self.wallet.pubkey()),
                &[self.wallet.as_ref()],
                recent_blockhash,
            );
            let signature = self.rpc.send_and_confirm_transaction(&tx).await?;
            println!("Closed position. Signature: {}", signature);

            let liquidity = result.quote.liquidity_delta;
            self.position = None;
            self.display_balances().await?;
            Ok((signature, liquidity))
        } else {
            Err(anyhow::anyhow!("No position to close"))
        }
    }

    pub async fn rebalance(&mut self, current_price: f64, amount: u64) -> Result<()> {
        let gas_reserve = 50_000_000;
        let position = self.get_position().await?;
        let whirlpool_account = self.client.rpc.get_account(&self.pool_address).await?;
        let whirlpool = Whirlpool::from_bytes(&whirlpool_account.data)
            .map_err(|e| anyhow::anyhow!("Failed to deserialize Whirlpool: {}", e))?;
        let in_range = is_position_in_range(
            whirlpool.sqrt_price.into(),
            position.tick_lower_index,
            position.tick_upper_index,
        );

        if !in_range {
            let (signature, liquidity) = self.close_position().await?;
            println!("Closed position with liquidity: {}", liquidity);

            let sol_mint = Pubkey::from_str("So11111111111111111111111111111111111111112")?;
            let primary_mint = if self.token_mint_a == sol_mint { self.token_mint_a } else { self.token_mint_b };
            let secondary_mint = if self.token_mint_a == sol_mint { self.token_mint_b } else { self.token_mint_a };

            let usd_value = amount as f64 * current_price / 1_000_000_000.0;
            let secondary_amount = (usd_value * 1_000_000_000.0) as u64;

            let adjusted_amount = amount - gas_reserve;
            let adjusted_usd_value = adjusted_amount as f64 * current_price / 1_000_000_000.0;
            let secondary_to_swap = (adjusted_usd_value * 1_000_000_000.0) as u64;

            self.swap_tokens(secondary_to_swap, secondary_mint, primary_mint).await?;

            let new_lower = current_price - 10.0;
            let new_upper = current_price + 10.0;
            println!("Opening new position at {}–{}", new_lower, new_upper);
            let param = IncreaseLiquidityParam::Liquidity(liquidity);
            let result = open_position_instructions(
                &self.rpc,
                self.pool_address,
                new_lower,
                new_upper,
                param,
                Some(100),
                Some(self.wallet.pubkey()),
            ).await?;

            let recent_blockhash = self.rpc.get_latest_blockhash().await?;
            let tx = Transaction::new_signed_with_payer(
                &result.instructions,
                Some(&self.wallet.pubkey()),
                &[self.wallet.as_ref()],
                recent_blockhash,
            );
            let signature = self.rpc.send_and_confirm_transaction(&tx).await?;
            println!("Opened new position. Signature: {}", signature);

            let (position_address, _) = get_position_address(&result.position_mint)?;
            self.position = Some(Position {
                lower_price: new_lower,
                upper_price: new_upper,
                mint_address: result.position_mint,
                address: position_address,
                tick_lower_index: position.tick_lower_index,
                tick_upper_index: position.tick_upper_index,
                liquidity,
            });
            self.display_balances().await?;
        }
        Ok(())
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
            let close_result = close_position_instructions(
                &self.rpc,
                position.mint_address,
                Some(100),
                Some(self.wallet.pubkey()),
            ).await?;
            let token_a_balance = close_result.quote.token_est_a as f64 / 10f64.powi(self.token_mint_a_decimals as i32);
            let token_b_balance = close_result.quote.token_est_b as f64 / 10f64.powi(self.token_mint_b_decimals as i32);
            println!(
                "Position Balances:\n- Token A ({:?}): {:.6}\n- Token B ({:?}): {:.6}",
                self.token_mint_a, token_a_balance, self.token_mint_b, token_b_balance
            );
        }
        Ok(())
    }

    pub async fn has_existing_position(&self) -> Result<bool> {
        let positions = fetch_positions_for_owner(&self.rpc, self.pool_address).await?;
        Ok(!positions.is_empty())
    }
}