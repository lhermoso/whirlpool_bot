use clap::{Arg, Command};

pub struct CliArgs {
    pub keypair_path: String,
    pub position_mint_address: String,
    pub interval: u64,
    pub invest: f64,
    pub pool_address: String,
}

pub fn parse_args() -> CliArgs {
    let matches = Command::new("whirlpool_bot")
        .arg(
            Arg::new("keypair-path")
                .long("keypair-path")
                .value_name("KEYPAIR_PATH")
                .default_value("wallet.json")
                .help("Path to the Solana keypair file"),
        )
        .arg(
            Arg::new("position-mint-address")
                .long("position-mint-address")
                .value_name("POSITION_MINT_ADDRESS")
                .default_value("")
                .help("Mint address of an existing position (optional)"),
        )
        .arg(
            Arg::new("interval")
                .long("interval")
                .value_name("INTERVAL")
                .default_value("60")
                .help("Polling interval in seconds"),
        )
        .arg(
            Arg::new("invest")
                .long("invest")
                .value_name("INVEST")
                .default_value("1.0")
                .help("Amount of SOL to invest in the pool"),
        )
        .arg(
            Arg::new("pool-address")
                .long("pool-address")
                .value_name("POOL_ADDRESS")
                .required(true)
                .help("Address of the Whirlpool pool to trade on (e.g., Czfq3xZZ...)"),
        )
        .get_matches();

    CliArgs {
        keypair_path: matches.get_one::<String>("keypair-path").unwrap().to_string(),
        position_mint_address: matches.get_one::<String>("position-mint-address").unwrap().to_string(),
        interval: matches.get_one::<String>("interval").unwrap().parse::<u64>().unwrap(),
        invest: matches.get_one::<String>("invest").unwrap().parse::<f64>().unwrap(),
        pool_address: matches.get_one::<String>("pool-address").unwrap().to_string(),
    }
}