use solana_sdk::signature::{Signer, read_keypair_file};
use std::env;
use std::path::Path;

pub fn load_wallet() -> Box<dyn Signer + Send + Sync> {
    let keypair_path = env::var("KEYPAIR_PATH").unwrap_or_else(|_| "wallet.json".to_string());
    
    println!("Loading wallet from: {}", keypair_path);
    
    let path = Path::new(&keypair_path);
    if !path.exists() {
        panic!("Wallet file not found at: {}. Please create a wallet file or set the KEYPAIR_PATH environment variable.", keypair_path);
    }
    
    let keypair = read_keypair_file(path)
        .unwrap_or_else(|e| panic!("Failed to read keypair from {}: {}", keypair_path, e));
    
    Box::new(keypair)
}