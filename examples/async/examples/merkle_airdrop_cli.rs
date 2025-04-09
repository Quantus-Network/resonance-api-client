/*
    Copyright 2024 Resonance Network
    Licensed under the Apache License, Version 2.0 (the "License");
    you may not use this file except in compliance with the License.
    You may obtain a copy of the License at

        http://www.apache.org/licenses/LICENSE-2.0

    Unless required by applicable law or agreed to in writing, software
    distributed under the License is distributed on an "AS IS" BASIS,
    WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
    See the License for the specific language governing permissions and
    limitations under the License.
*/

//! CLI tool for testing the merkle-airdrop pallet in the Resonance Network.
//! Provides functionality to:
//! 1. Generate Merkle trees, roots, and proofs from a list of claims
//! 2. Create new airdrops with a Merkle root
//! 3. Fund existing airdrops with a specified amount
//! 4. Claim rewards from airdrops using Merkle proofs

use clap::{Parser, Subcommand};
use substrate_api_client::{
    Api, XtStatus,
    rpc::JsonrpseeClient,
    ac_primitives::ResonanceRuntimeConfig,
    SubmitAndWatch,
    ac_compose_macros::compose_extrinsic,
    ac_primitives::HashTrait,
};
use dilithium_crypto::pair::{dilithium_bob};
use sp_core::H256;
use sp_core::{ crypto::{Ss58Codec}, sr25519 };
use std::fmt;
use log::info;
use codec::Encode;
use poseidon_resonance::PoseidonHasher;

#[derive(Debug)]
enum CliError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Hex(hex::FromHexError),
    Api(substrate_api_client::Error),
    Rpc(substrate_api_client::rpc::Error),
    Custom(String),
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CliError::Io(e) => write!(f, "IO error: {}", e),
            CliError::Json(e) => write!(f, "JSON error: {}", e),
            CliError::Hex(e) => write!(f, "Hex decoding error: {}", e),
            CliError::Api(e) => write!(f, "API error: {:?}", e),
            CliError::Rpc(e) => write!(f, "RPC error: {:?}", e),
            CliError::Custom(s) => write!(f, "{}", s),
        }
    }
}

impl std::error::Error for CliError {}

impl From<std::io::Error> for CliError {
    fn from(e: std::io::Error) -> Self {
        CliError::Io(e)
    }
}

impl From<serde_json::Error> for CliError {
    fn from(e: serde_json::Error) -> Self {
        CliError::Json(e)
    }
}

impl From<hex::FromHexError> for CliError {
    fn from(e: hex::FromHexError) -> Self {
        CliError::Hex(e)
    }
}

impl From<substrate_api_client::Error> for CliError {
    fn from(e: substrate_api_client::Error) -> Self {
        CliError::Api(e)
    }
}

impl From<substrate_api_client::rpc::Error> for CliError {
    fn from(e: substrate_api_client::rpc::Error) -> Self {
        CliError::Rpc(e)
    }
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[command(subcommand)]
    command: Command,

    #[arg(short, long, default_value = "ws://127.0.0.1:9944")]
    node_url: String,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Create a merkle tree and output root and proofs
    GenerateMerkleTree {
        #[arg(short, long)]
        input_file: String,
        
        #[arg(short, long)]
        output_file: Option<String>,
    },
    
    /// Create a new airdrop with the given merkle root
    CreateAirdrop {
        #[arg(short, long)]
        merkle_root: String,
    },
    
    /// Fund an existing airdrop
    FundAirdrop {
        #[arg(short, long)]
        id: u32,
        
        #[arg(short, long)]
        amount: u128,
    },
    
    /// Claim from an airdrop
    Claim {
        #[arg(short, long)]
        id: u32,
        
        #[arg(short, long)]
        amount: u128,
        
        #[arg(short, long)]
        proofs: Vec<String>,
    },
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct Claim {
    address: String,
    amount: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    encoded_account: Option<Vec<u8>>,
}

type BalanceOf = u128;

fn calculate_leaf_hash_poseidon(
    account: &sr25519::Public,
    amount: BalanceOf,
) -> [u8; 32] {
    let account_bytes = account.encode();
    let amount_bytes = amount.encode();
    let combined = [account_bytes.as_slice(), amount_bytes.as_slice()].concat();
    let mut output = [0u8; 32];
    output.copy_from_slice(&PoseidonHasher::hash(&combined)[..]);
    output
}

fn calculate_parent_hash(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let combined = if left < right {
        [&left[..], &right[..]].concat()
    } else {
        [&right[..], &left[..]].concat()
    };

    let mut output = [0u8; 32];
    output.copy_from_slice(&PoseidonHasher::hash(&combined)[..]);
    output
}

fn build_merkle_tree(claims: &[Claim]) -> (Vec<Vec<[u8; 32]>>, [u8; 32]) {
    let mut layers = Vec::new();
    
    // Create leaf layer
    let mut current_layer: Vec<[u8; 32]> = claims
        .iter()
        .map(|claim| {
            let account_id = sr25519::Public::from_ss58check(&claim.address)
                .expect("Invalid SS58 address");
            
            // Parse amount string to u128
            let amount = claim.amount.parse::<u128>()
                .expect("Invalid amount format");
            calculate_leaf_hash_poseidon(&account_id, amount)
        })
        .collect();
    layers.push(current_layer.clone());

    // Build tree layers
    while current_layer.len() > 1 {
        let mut next_layer = Vec::new();
        for chunk in current_layer.chunks(2) {
            if chunk.len() == 2 {
                next_layer.push(calculate_parent_hash(&chunk[0], &chunk[1]));
            } else {
                next_layer.push(chunk[0]);
            }
        }
        layers.push(next_layer.clone());
        current_layer = next_layer;
    }

    (layers, current_layer[0])
}

impl Args {
    fn generate_merkle_tree(&self, input_file: &str) -> Result<(), CliError> {
        let file = std::fs::File::open(input_file)?;
        let mut claims: Vec<Claim> = serde_json::from_reader(file)?;
        
        // Pre-process claims to store encoded account bytes
        for claim in &mut claims {
            let account_id = sr25519::Public::from_ss58check(&claim.address)
                .map_err(|e| CliError::Custom(format!("Invalid SS58 address: {}", e)))?;
            claim.encoded_account = Some(account_id.encode());
        }
        
        let (layers, root) = build_merkle_tree(&claims);
        
        // Generate proofs for each claim
        let mut proofs = Vec::new();
        for (i, claim) in claims.iter().enumerate() {
            let mut proof = Vec::new();
            let mut current_idx = i;
            
            for (layer_idx, _layer) in layers.iter().enumerate().skip(1) {
                let sibling_idx = if current_idx % 2 == 0 {
                    current_idx + 1
                } else {
                    current_idx - 1
                };
                
                if sibling_idx < layers[layer_idx - 1].len() {
                    proof.push(layers[layer_idx - 1][sibling_idx]);
                }
                
                current_idx /= 2;
            }
            
            proofs.push(serde_json::json!({
                "address": claim.address,
                "amount": claim.amount,
                "proof": proof.iter().map(|p| {
                    serde_json::json!({ "hash": format!("0x{}", hex::encode(p)), })
                }).collect::<Vec<_>>()
            }));
        }
        
        let output = serde_json::json!({
            "root": format!("0x{}", hex::encode(root)),
            "claims": proofs
        });
        
        println!("{}", serde_json::to_string_pretty(&output)?);
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), CliError> {
    env_logger::init();
    let args = Args::parse();
    
    match &args.command {
        Command::GenerateMerkleTree { input_file, output_file: _ } => {
            info!("Generating Merkle tree from {}", input_file);
            args.generate_merkle_tree(input_file)?;
            Ok(())
        },
        
        Command::CreateAirdrop { merkle_root } => {
            info!("Connecting to node at {}", args.node_url);
            let client = JsonrpseeClient::new(&args.node_url).await?;
            let mut api = Api::<ResonanceRuntimeConfig, _>::new(client).await?;
            
            // Remove 0x prefix if present
            let merkle_root = merkle_root.trim_start_matches("0x");
            let merkle_bytes = hex::decode(merkle_root)?;
            
            // Ensure we have exactly 32 bytes
            if merkle_bytes.len() != 32 {
                return Err(CliError::Custom(format!("Merkle root must be exactly 32 bytes, got {}", merkle_bytes.len())));
            }
            
            let mut root = [0u8; 32];
            root.copy_from_slice(&merkle_bytes);
            let root = H256::from(root);
            
            info!("Creating airdrop with merkle root: {}", hex::encode(root.as_bytes()));
            
            let signer = dilithium_bob();
            api.set_signer(signer.clone().into());
            info!("Using signer: {:?}", signer.public());
            
            let api_ref = api.clone();
            let xt = compose_extrinsic!(
                api_ref,
                "MerkleAirdrop",
                "create_airdrop",
                root
            ).ok_or_else(|| CliError::Custom("Failed to create extrinsic".to_string()))?;
            
            info!("Submitting createAirdrop transaction...");
            let result = api.submit_and_watch_extrinsic_until(xt, XtStatus::InBlock).await?;
            info!("Transaction included in block: {:?}", result);
            
            Ok(())
        },
        
        Command::FundAirdrop { id, amount } => {
            info!("Connecting to node at {}", args.node_url);
            let client = JsonrpseeClient::new(&args.node_url).await?;
            let mut api = Api::<ResonanceRuntimeConfig, _>::new(client).await?;
            
            info!("Funding airdrop {} with amount {}", id, amount);
            
            let signer = dilithium_bob();
            api.set_signer(signer.clone().into());
            info!("Using signer: {:?}", signer.public());
            
            // Store the cloned API
            let api_ref = api.clone();
            // Create and sign the extrinsic
            let xt = compose_extrinsic!(
                api_ref,
                "MerkleAirdrop",
                "fund_airdrop",
                id,
                amount
            ).ok_or_else(|| CliError::Custom("Failed to create extrinsic".to_string()))?;
            
            // Submit and wait for inclusion
            info!("Submitting fundAirdrop transaction...");
            let result = api.submit_and_watch_extrinsic_until(xt, XtStatus::InBlock).await?;
            info!("Transaction included in block: {:?}", result);
            
            Ok(())
        },
        
        Command::Claim { id, amount, proofs } => {
            info!("Connecting to node");
            let client = JsonrpseeClient::new(&args.node_url).await?;
            let mut api = Api::<ResonanceRuntimeConfig, _>::new(client).await?;
            
            info!("Claiming from airdrop {} for amount {}", id, amount);
            
            let signer = dilithium_bob();
            info!("Signer public key: {:?}", signer.public());
            
            let account_id = signer.public();
            info!("Using account ID: {:?}", account_id);
            
            // Set the signer
            api.set_signer(signer.clone().into());
            info!("Using signer: {:?}", signer.public());
            
            // Convert proof strings to [u8; 32] arrays
            let proof_bytes: Vec<[u8; 32]> = proofs
                .iter()
                .map(|p| {
                    let p = p.trim_start_matches("0x");
                    hex::decode(p)
                        .map_err(|e| CliError::Custom(format!("Invalid hex in proof '{}': {}", p, e)))
                        .and_then(|bytes| {
                            bytes.try_into().map_err(|_| {
                                CliError::Custom(format!("Proof '{}' is not 32 bytes long", p))
                            })
                        })
                })
                .collect::<Result<_, _>>()?; // Collect results, propagating errors
            info!("Proof bytes: {:?}", proof_bytes);
            
            // for debugging
            let encoded_account = account_id.encode();
            info!("Encoded account bytes: {:?}", hex::encode(&encoded_account));
            
            let api_ref = api.clone();
            // Create and sign the extrinsic
            let xt = compose_extrinsic!(
                api_ref,
                "MerkleAirdrop",
                "claim",
                id,
                amount,
                proof_bytes
            ).ok_or_else(|| CliError::Custom("Failed to create extrinsic".to_string()))?;

            info!("Created extrinsic: {:?}", xt);
            
            info!("Submitting claim transaction...");
            let result = api.submit_and_watch_extrinsic_until(xt, XtStatus::InBlock).await?;
            info!("Transaction included in block: {:?}", result.block_hash);

            Ok(())
        },
    }
} 
