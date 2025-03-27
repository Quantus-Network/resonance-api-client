use codec::Encode;
use poseidon_resonance::PoseidonHasher;
use sp_core::crypto::AccountId32;
use sp_core::twox_128;
use sp_state_machine::read_proof_check;
use sp_trie::StorageProof;
use trie_db::node::{Node, NodeHandle};
use trie_db::{TrieLayout, NodeCodec};
use substrate_api_client::ac_primitives::{HashTrait, ResonanceRuntimeConfig, StorageKey};
use substrate_api_client::{Api, GetStorage, GetChainInfo};
use substrate_api_client::rpc::JsonrpseeClient;
use substrate_api_client::runtime_api::AccountNonceApi;

pub async fn verify_transfer_proof(
    api: Api::<ResonanceRuntimeConfig, JsonrpseeClient>,
    from: AccountId32,
    to: AccountId32,
    amount: u128,
) -> bool {
    let block_hash = api.get_block_hash(None).await.unwrap().unwrap();
    // This gives the chain time to fully process the new block, not sure if it's 100% necessary now
    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

    let nonce = api.runtime_api().account_nonce(from.clone(), None).await.unwrap();
    let key_tuple = (nonce, from, to, amount);
    println!("[+] Transaction nonce: {nonce:?} key: {key_tuple:?}");

    let pallet_prefix = twox_128("Balances".as_bytes());
    let storage_prefix = twox_128("TransferProof".as_bytes());
    let encoded_key = key_tuple.encode();
    let key_hash = <PoseidonHasher as HashTrait>::hash(&encoded_key);

    let mut correct_storage_key = [&pallet_prefix[..], &storage_prefix[..], key_hash.as_ref()].concat();
    let storage_key = StorageKey(correct_storage_key.clone());

    let proof = api.get_storage_proof_by_keys(vec![storage_key.clone()], Some(block_hash))
        .await
        .unwrap()
        .unwrap();

    let mut proof_as_u8: Vec<Vec<u8>> = proof.proof
        .iter() // Iterate over the Vec<Bytes>
        .map(|bytes| bytes.as_ref().to_vec()) // Convert each Bytes to Vec<u8>
        .collect::<Vec<_>>(); // Collect into Vec<Vec<u8>>

    for (i, node_data) in proof_as_u8.iter().enumerate() {
        match <sp_trie::LayoutV1<PoseidonHasher> as TrieLayout>::Codec::decode(node_data) {
            Ok(node) => {
                match &node {
                    Node::Empty => log::info!("Proof node {}: Empty", i),
                    Node::Leaf(partial, value) => {
                        let nibbles: Vec<u8> = partial.right_iter().collect();
                        log::info!("Proof node {}: Leaf, partial: {:?}, value: {:?}",
                        i, hex::encode(&nibbles), value);
                    },
                    Node::Extension(partial, _) => {
                        let nibbles: Vec<u8> = partial.right_iter().collect();
                        log::info!("Proof node {}: Extension, partial: {:?}", i, hex::encode(&nibbles));
                    },
                    Node::Branch(children, value) => {
                        log::info!("Proof node {}: Branch, value: {:?}", i, value);
                        for (j, child) in children.iter().enumerate() {
                            if let Some(child) = child {
                                log::info!("  Child {}: {:?}", j, child);
                            }
                        }
                    },
                    Node::NibbledBranch(partial, children, value) => {
                        let nibbles: Vec<u8> = partial.right_iter().collect();
                        let children = children.iter()
                            .filter_map(|x| x.as_ref()
                                .map(|val| match val {
                                    NodeHandle::Hash(h) => hex::encode(h),
                                    NodeHandle::Inline(i) => hex::encode(i)
                                })
                            ).collect::<Vec<String>>();
                        log::info!("Proof node {}: NibbledBranch, partial: {:?}, value: {:?}, children: {:?}",
                        i, hex::encode(&nibbles), value, children);
                    },
                }
            },
            Err(e) => log::info!("Failed to decode proof node {}: {:?}", i, e),
        }
    }

    println!("Storage proof at block {:?} {:?}", block_hash, proof_as_u8.iter().map(|x| hex::encode(x)).collect::<Vec<_>>());

    println!("Storage key: {:?} {:?}", hex::encode(&storage_key), storage_key);

    let header = api.get_header(Some(block_hash)).await.unwrap().unwrap();
    let state_root = header.state_root;
    println!("Header: {:?} State root: {:?}", header, state_root);
    let expected_value = true.encode();
    println!("Expected value: {:?}", expected_value);

    let storage_value = api
        .get_storage_by_key::<bool>(storage_key.clone(), Some(block_hash))
        .await
        .unwrap();
    println!("Storage value: {:?}", storage_value);

    let mut items = Vec::new();
    items.push(correct_storage_key.clone());

    let storage_proof = StorageProof::new(proof_as_u8.clone());
    let result = read_proof_check::<PoseidonHasher, &Vec<Vec<u8>>>(state_root, storage_proof, &items);

    // Don't use verify_proof it assumes different prefixes, not empty
    // let result = verify_proof::<LayoutV1<PoseidonHasher>, _, _, _>(
    // 	&state_root, &proof_as_u8, items.iter());

    match result {
        Ok(map) => {
            match map.get(&correct_storage_key) {
                Some(Some(value)) => {
                    if value == &expected_value {
                        println!("Proof verified {:?} {:?}", correct_storage_key, value);
                        true
                    } else {
                        println!("Proof failed to verify {:?} {:?}", correct_storage_key, value);
                        false
                    }
                }
                Some(None) => {
                    println!("Proof returned None for key {:?}", correct_storage_key);
                    false
                }
                None => {
                    println!("Key {:?} not found in proof", correct_storage_key);
                    false
                }
            }
        },
        Err(e) => {
            println!("Failed to check proof: {:?}", e);
            false
        },
    }

    // Testing that verification fails if storage key is edited (TODO: turn into test)
    // let mut correct_storage_key2 = correct_storage_key.clone();
    // correct_storage_key2[0] = 2;
    //
    // let storage_proof = StorageProof::new(proof_as_u8.clone());
    // let result = read_proof_check::<PoseidonHasher, &Vec<Vec<u8>>>(state_root, storage_proof, &items);
    //
    // match result {
    //     Ok(map) => {
    //         match map.get(&correct_storage_key2) {
    //             Some(Some(value)) => {
    //                 if value == &expected_value {
    //                     println!("Proof verified {:?} {:?}", correct_storage_key2, value);
    //                 } else {
    //                     println!("Proof failed to verify {:?} {:?}", correct_storage_key2, value);
    //                 }
    //             }
    //             Some(None) => {
    //                 println!("Proof returned None for key {:?}", correct_storage_key2);
    //             }
    //             None => {
    //                 println!("Key {:?} not found in proof", correct_storage_key2);
    //             }
    //         }
    //     },
    //     Err(e) => println!("Failed to check proof: {:?}", e),
    // }

    // Testing that editing proof causing verification to fail (TODO: turn into test)
    // let mut proof_as_u82 = proof_as_u8.clone();
    // proof_as_u82[0][0] += 2;
    //
    // let storage_proof = StorageProof::new(proof_as_u82.clone());
    // let result = read_proof_check::<PoseidonHasher, &Vec<Vec<u8>>>(state_root, storage_proof, &items);
    //
    // match result {
    //     Ok(map) => {
    //         match map.get(&correct_storage_key) {
    //             Some(Some(value)) => {
    //                 if value == &expected_value {
    //                     println!("Proof verified {:?} {:?}", correct_storage_key, value);
    //                 } else {
    //                     println!("Proof failed to verify {:?} {:?}", correct_storage_key, value);
    //                 }
    //             }
    //             Some(None) => {
    //                 println!("Proof returned None for key {:?}", correct_storage_key);
    //             }
    //             None => {
    //                 println!("Key {:?} not found in proof", correct_storage_key);
    //             }
    //         }
    //     },
    //     Err(e) => println!("Failed to check proof: {:?}", e),
    // }
}

fn main() {}