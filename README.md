# Whirlpool Bot

A trading bot for automated liquidity provision and rebalancing on Orca Whirlpools on the Solana blockchain.

## Overview

This bot automates liquidity provision for Orca Whirlpools by:
- Opening positions in specified liquidity pools
- Monitoring prices at regular intervals
- Rebalancing positions when they go out of range
- Managing wallet balances and swapping tokens when needed

The bot is designed to maintain an active position around the current market price, allowing you to earn trading fees continuously even as prices fluctuate.

## Features

- 💰 Automated liquidity provision
- 🔄 Price-based position rebalancing
- 💱 Automatic token swapping when needed
- 📊 Support for custom price range percentages
- 🌐 Mainnet and Devnet support
- 📈 Detailed logging of all operations

## Prerequisites

- Rust (latest stable version)
- Solana CLI tools
- A Solana wallet with SOL and your target tokens
- Access to a Solana RPC endpoint

## Installation

1. Clone the repository:
```bash
git clone https://github.com/lhermoso/whirlpool_bot.git
cd whirlpool_bot
```

2. Build the project:
```bash
cargo build --release
```

## Setup

1. Create a `.env` file in the project root with your RPC endpoints:
```
RPC_URL=https://api.mainnet-beta.solana.com
RPC_DEV_URL=https://api.devnet.solana.com
```

2. Place your Solana wallet keypair in a file named `wallet.json` in the project root (or specify a different path with the CLI arguments).

## Usage

The bot can be run using the following command structure:

```bash
cargo run --release -- --pool-address <POOL_ADDRESS> [OPTIONS]
```

### Required Arguments

- `--pool-address <ADDRESS>`: The address of the Whirlpool pool to trade on

### Optional Arguments

- `--keypair-path <PATH>`: Path to your Solana keypair file (default: `wallet.json`)
- `--position-mint-address <ADDRESS>`: Mint address of an existing position (optional)
- `--interval <SECONDS>`: Polling interval in seconds (default: 60)
- `--invest <AMOUNT>`: Amount of SOL to invest in the pool (default: 1.0)
- `--range-percentage <PERCENTAGE>`: Price range as a percentage (default: 2.0, means ±1% from current price)
- `--network <NETWORK>`: Solana network to use, either "mainnet" or "devnet" (default: mainnet)

## Examples

### Basic Usage

```bash
# Invest 1 SOL in the specified pool with default settings
cargo run --release -- --pool-address Czfq3xZZ123456789ABCDEFG
```

### Advanced Usage

```bash
# Invest 0.5 SOL with a 5% range, checking every 30 seconds on devnet
cargo run --release -- \
  --pool-address Czfq3xZZ123456789ABCDEFG \
  --invest 0.5 \
  --range-percentage 5.0 \
  --interval 30 \
  --network devnet
```

## How It Works

The bot follows these steps:

1. **Initial Position**: Opens a position in the specified pool with your investment amount.
2. **Price Monitoring**: Continuously monitors the current price at the specified interval.
3. **Range Check**: Determines if the current price is within your position's range.
4. **Rebalancing**: If the price moves out of range, the bot:
   - Closes your existing position
   - Opens a new position centered around the current price
   - Uses the same percentage range for the new position

## Wallet Balance Management

The bot includes smart wallet balance management:
- Maintains a gas reserve (0.05 SOL) for transaction fees
- Can swap between pool tokens to ensure sufficient balance for operations
- Displays current wallet and position balances

## What Is A Whirlpool?

Orca Whirlpools are concentrated liquidity pools on Solana, similar to Uniswap v3 on Ethereum. They allow liquidity providers to specify price ranges for their assets, offering potentially higher capital efficiency compared to traditional constant product pools.

## Troubleshooting

### Common Issues

- **Insufficient SOL**: Ensure your wallet has enough SOL for both investment and transaction fees.
- **RPC Errors**: If you encounter RPC errors, try using a different RPC endpoint in your `.env` file.
- **Position Creation Failures**: Some pools may have restrictions or may not be properly initialized. Try with a different pool.

### Logs

The bot provides detailed logs of all operations. To increase log verbosity, set the `RUST_LOG` environment variable:

```bash
RUST_LOG=debug cargo run --release -- --pool-address <POOL_ADDRESS>
```

## License

This project is licensed under the MIT License - see the LICENSE file for details.

## Disclaimer

This bot is provided as-is with no guarantees. Trading cryptocurrencies involves risk, and you should not invest more than you can afford to lose. Always test thoroughly on devnet before using on mainnet. 