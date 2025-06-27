use codec::Encode;
use poseidon_resonance::PoseidonHasher;
use sp_core::{crypto::AccountId32, twox_128, Hasher, H256};
use sp_state_machine::read_proof_check;
use sp_trie::StorageProof;
use substrate_api_client::{
	ac_primitives::{DefaultRuntimeConfig, HashTrait, QuantusRuntimeConfig, StorageKey},
	rpc::JsonrpseeClient,
	runtime_api::AccountNonceApi,
	Api, GetChainInfo, GetStorage,
};
use trie_db::{
	node::{Node, NodeHandle},
	NodeCodec, TrieLayout,
};

/// Function to compute the leaf hash for the TransferProof storage map key.
/// Parameters:
/// - tx_count: The global transfer count
/// - from: The sender's AccountId.
/// - to: The recipient's AccountId.
/// - amount: The balance amount transferred.
/// Returns: The hashed leaf as a `T::Hash`.
pub fn compute_transfer_proof_leaf(
	tx_count: u64,
	from: &AccountId32,
	to: &AccountId32,
	amount: u128,
) -> [u8; 32] {
	// Step 1: Encode the key components into a single byte vector
	let mut key_bytes = Vec::new();
	key_bytes.extend_from_slice(&tx_count.encode());
	key_bytes.extend_from_slice(&from.encode());
	key_bytes.extend_from_slice(&to.encode());
	key_bytes.extend_from_slice(&amount.encode());
	// Step 2: Hash the concatenated bytes using PoseidonHasher
	PoseidonHasher::hash_storage::<AccountId32>(&key_bytes)
}

// Function to check that the 24 byte suffix of the leaf hash is the last [-32, -8] bytes of the leaf node
pub fn check_leaf(leaf_hash: &[u8; 32], leaf_node: Vec<u8>) -> bool {
	let hash_suffix = &leaf_hash[8..32];
	let node_suffix = &leaf_node[leaf_node.len() - 32..leaf_node.len() - 8];
	log::debug!(
		"Checking leaf hash suffix: {:?} against node suffix: {:?}",
		hex::encode(hash_suffix),
		hex::encode(node_suffix)
	);
	log::debug!("leaf_node: {:?}", hex::encode(leaf_node.clone()));

	hash_suffix == node_suffix
}

pub async fn verify_transfer_proof(
	api: Api<QuantusRuntimeConfig, JsonrpseeClient>,
	from: AccountId32,
	to: AccountId32,
	amount: u128,
) -> bool {
	let block_hash = api.get_block_hash(None).await.unwrap().unwrap();
	// This gives the chain time to fully process the new block, not sure if it's 100% necessary now
	tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
	let transfer_count: u64 = api
		.get_storage("Balances", "TransferCount", None)
		.await
		.unwrap()
		.unwrap_or(0u64);
	let key_tuple = (transfer_count, from.clone(), to.clone(), amount);
	log::info!("[+] Transfer count: {transfer_count:?} key: {key_tuple:?}");
	let leaf_hash =
		compute_transfer_proof_leaf(transfer_count - 1, &from.clone(), &to.clone(), amount);
	log::info!(
		"🍀 Leaf hash: {:?} From: {:?} To: {:?} Amount: {:?}",
		hex::encode(leaf_hash),
		from.encode(),
		to.encode(),
		amount.encode()
	);

	let pallet_prefix = twox_128("Balances".as_bytes());
	let storage_prefix = twox_128("TransferProof".as_bytes());
	let correct_storage_key =
		[&pallet_prefix[..], &storage_prefix[..], leaf_hash.as_ref()].concat();
	let storage_key = StorageKey(correct_storage_key.clone());

	let proof = api
		.get_storage_proof_by_keys(vec![storage_key.clone()], Some(block_hash))
		.await
		.unwrap()
		.unwrap();

	let proof_as_u8: Vec<Vec<u8>> = proof
		.proof
		.iter() // Iterate over the Vec<Bytes>
		.map(|bytes| bytes.as_ref().to_vec()) // Convert each Bytes to Vec<u8>
		.collect::<Vec<_>>(); // Collect into Vec<Vec<u8>>

	let leaf_checked = check_leaf(&leaf_hash, proof_as_u8[proof_as_u8.len() - 1].clone());
	let check_string = if leaf_checked { "✅" } else { "⚔️" };
	log::info!("🍀 Leaf check: {check_string}");

	for (i, node_data) in proof_as_u8.iter().enumerate() {
		let node_hash = <PoseidonHasher as Hasher>::hash(node_data);
		match <sp_trie::LayoutV1<PoseidonHasher> as TrieLayout>::Codec::decode(node_data) {
			Ok(node) =>
				match &node {
					Node::Empty => log::info!("Proof node {}: Empty", i),
					Node::Leaf(partial, value) => {
						let nibbles: Vec<u8> = partial.right_iter().collect();
						log::info!("Proof node {}: Leaf, partial: {:?}, value: {:?} hash: {:?} bytes: {:?}",
                        i, hex::encode(&nibbles), value, node_hash, hex::encode(&node_data));
					},
					Node::Extension(partial, _) => {
						let nibbles: Vec<u8> = partial.right_iter().collect();
						log::info!(
							"Proof node {}: Extension, partial: {:?} hash: {:?} bytes: {:?}",
							i,
							hex::encode(&nibbles),
							node_hash,
							hex::encode(&node_data)
						);
					},
					Node::Branch(children, value) => {
						log::info!(
							"Proof node {}: Branch, value: {:?} hash: {:?} bytes: {:?}",
							i,
							value,
							node_hash,
							hex::encode(&node_data)
						);
						for (j, child) in children.iter().enumerate() {
							if let Some(child) = child {
								log::info!("  Child {}: {:?}", j, child);
							}
						}
					},
					Node::NibbledBranch(partial, children, value) => {
						let nibbles: Vec<u8> = partial.right_iter().collect();
						let children = children
							.iter()
							.filter_map(|x| {
								x.as_ref().map(|val| match val {
									NodeHandle::Hash(h) => hex::encode(h),
									NodeHandle::Inline(i) => hex::encode(i),
								})
							})
							.collect::<Vec<String>>();
						log::info!("Proof node {}: NibbledBranch, partial: {:?}, value: {:?}, children: {:?} hash: {:?} bytes: {:?}",
                        i, hex::encode(&nibbles), value, children, node_hash, hex::encode(&node_data));
					},
				},
			Err(e) => log::info!("Failed to decode proof node {}: {:?}", i, e),
		}
	}

	println!(
		"Storage proof at block {:?} {:?}",
		block_hash,
		proof_as_u8.iter().map(hex::encode).collect::<Vec<_>>()
	);

	println!("Storage key: {:?} {:?}", hex::encode(&storage_key), storage_key);

	let header = api.get_header(Some(block_hash)).await.unwrap().unwrap();
	let state_root = header.state_root;

	prepare_proof_for_circuit(proof_as_u8.clone(), hex::encode(state_root));

	println!("Header: {:?} State root: {:?}", header, state_root);
	let expected_value = ().encode();
	println!("Expected value: {:?}", expected_value);

	let storage_value = api
		.get_storage_by_key::<()>(storage_key.clone(), Some(block_hash))
		.await
		.unwrap();
	println!("Storage value: {:?}", storage_value);

	let mut items = Vec::new();
	items.push(correct_storage_key.clone());

	let storage_proof = StorageProof::new(proof_as_u8.clone());
	let result =
		read_proof_check::<PoseidonHasher, &Vec<Vec<u8>>>(state_root, storage_proof, &items);

	// Don't use verify_proof it assumes different prefixes, not empty
	// let result = verify_proof::<LayoutV1<PoseidonHasher>, _, _, _>(
	// 	&state_root, &proof_as_u8, items.iter());

	match result {
		Ok(map) => match map.get(&correct_storage_key) {
			Some(Some(value)) =>
				if value == &expected_value {
					println!("Proof verified {:?} {:?}", correct_storage_key, value);
					true
				} else {
					println!("Proof failed to verify {:?} {:?}", correct_storage_key, value);
					false
				},
			Some(None) => {
				println!("Proof returned None for key {:?}", correct_storage_key);
				false
			},
			None => {
				println!("Key {:?} not found in proof", correct_storage_key);
				false
			},
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

fn prepare_proof_for_circuit(
	proof: Vec<Vec<u8>>,
	state_root: String,
) -> (Vec<String>, Vec<String>, Vec<(String, String)>) {
	let mut hashes = Vec::<String>::new();
	let mut bytes = Vec::<String>::new();
	let mut parts = Vec::<(String, String)>::new();
	let mut storage_proof = Vec::<String>::new();
	for node_data in proof.iter() {
		let hash = hex::encode(<PoseidonHasher as Hasher>::hash(node_data));
		let node_bytes = hex::encode(node_data);
		if hash == state_root {
			storage_proof.push(node_bytes);
		} else {
			// don't put the hash in if it is the root
			hashes.push(hash);
			bytes.push(node_bytes.clone());
		}
	}

	log::info!("Finished constructing bytes and hashes vectors {:?} {:?}", bytes, hashes);
	let mut ordered_hashes = Vec::<String>::new();
	let mut indices = Vec::<usize>::new();
	while !hashes.is_empty() {
		for i in (0..hashes.len()).rev() {
			let hash = hashes[i].clone();
			if let Some(last) = storage_proof.last() {
				if let Some(index) = last.find(&hash) {
					let (left, right) = last.split_at(index);
					indices.push(index);
					parts.push((left.to_string(), right.to_string()));
					storage_proof.push(bytes[i].clone());
					ordered_hashes.push(hash.clone());
					hashes.remove(i);
					bytes.remove(i);
				}
			}
		}
	}

	log::info!(
		"Storage proof generated: {:?} {:?} {:?} {:?}",
		&storage_proof,
		parts,
		ordered_hashes,
		indices
	);

	for (i, _) in storage_proof.iter().enumerate() {
		if i == parts.len() {
			break
		}
		let part = parts[i].clone();
		let hash = ordered_hashes[i].clone();
		// log::info!("{:?} =? {:?}", node[index..index + 64], hash);
		if part.1[..64] != hash {
			log::error!("storage proof index incorrect {:?} != {:?}", part.1, hash);
		} else {
			log::info!("storage proof index correct: {:?}", part.0.len());
		}
	}

	(storage_proof, ordered_hashes, parts)
}

#[allow(unused)]
fn main() {}
