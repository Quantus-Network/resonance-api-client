//! This example shows how to use the compose_extrinsic macro to create a sudo extrinsic
//! that adds and removes a member to the technical committee.

use codec::Decode;
use dilithium_crypto::{pair::crystal_alice, types::ResonancePair};
use rand::RngCore;
use sp_core::{crypto::Ss58Codec, Pair};
use substrate_api_client::{
	ac_compose_macros::{compose_call, compose_extrinsic},
	ac_primitives::{quantus_runtime_config::QuantusRuntimeConfig, Config},
	rpc::JsonrpseeClient,
	Api, GetStorage, SubmitAndWatch, XtStatus,
};

// AccountId type of the runtime.
type AccountId = <QuantusRuntimeConfig as Config>::AccountId;
type Address = <QuantusRuntimeConfig as Config>::Address;

async fn get_members(api: &Api<QuantusRuntimeConfig, JsonrpseeClient>) -> Vec<AccountId> {
	let key_prefix = api.get_storage_map_key_prefix("TechCollective", "Members").await.unwrap();
	let member_keys = api.get_storage_keys_paged(Some(key_prefix), 1000, None, None).await.unwrap();

	member_keys
		.into_iter()
		.map(|key| AccountId::decode(&mut &key.0[40..]).unwrap())
		.collect()
}

#[tokio::main]
async fn main() {
	env_logger::init();

	// Initialize api and set the signer (alice)
	let from = crystal_alice();
	let client = JsonrpseeClient::new("ws://127.0.0.1:9944").await.unwrap();
	let mut api = Api::<QuantusRuntimeConfig, _>::new(client).await.unwrap();
	api.set_signer(from.into());

	// Generate a random member.
	// `ResonancePair::generate()` panics because the underlying implementation may use rejection
	// sampling, and the default `sp_core::Pair::generate()` doesn't handle that.
	// We implement a loop to retry until a valid key is generated from a random seed.
	let (new_member, _) = loop {
		let mut seed = [0u8; 32];
		rand::thread_rng().fill_bytes(&mut seed);
		if let Ok(pair) = ResonancePair::from_seed_slice(&seed) {
			break (pair, seed.to_vec());
		}
	};
	let new_member_account_id: AccountId = new_member.public().into();
	let new_member_address: Address = new_member_account_id.clone().into();

	println!("---------------------------------------------------------------------------------");
	println!("Generated random member: {}", new_member_account_id.to_ss58check());
	println!("---------------------------------------------------------------------------------");

	// 1. Add member
	println!("");
	println!("Adding member to TechCollective: {}", new_member_account_id.to_ss58check());

	// Create a call to add the member.
	let add_member_call =
		compose_call!(api.metadata(), "TechCollective", "add_member", new_member_address.clone())
			.unwrap();

	let xt = compose_extrinsic!(api, "Sudo", "sudo", add_member_call).unwrap();

	let block_hash = api
		.submit_and_watch_extrinsic_until(xt, XtStatus::InBlock)
		.await
		.unwrap()
		.block_hash;
	println!("[+] Extrinsic included in block with hash {:?}", block_hash);

	// Verify that the member has been added.
	let members = get_members(&api).await;
	assert!(members.contains(&new_member_account_id));
	println!("[+] Member {} has been added successfully.", new_member_account_id.to_ss58check());
	println!(
		"[+] Current members: {:?}",
		members.iter().map(|m| m.to_ss58check()).collect::<Vec<String>>()
	);

	// 2. Remove member
	println!("");
	println!("-----------------------------------");
	println!("Removing {} from the Technical Committee", new_member_account_id.to_ss58check());
	println!("-----------------------------------");

	// Create a call to remove the member.
	let remove_member_call = compose_call!(
		api.metadata(),
		"TechCollective",
		"remove_member",
		new_member_address.clone()
	)
	.unwrap();
	let xt = compose_extrinsic!(api, "Sudo", "sudo", remove_member_call).unwrap();

	// Submit the extrinsic.
	let block_hash = api
		.submit_and_watch_extrinsic_until(xt, XtStatus::InBlock)
		.await
		.unwrap()
		.block_hash;
	println!("[+] Extrinsic included in block with hash {:?}", block_hash);

	// Verify that the member has been removed.
	let members = get_members(&api).await;
	assert!(!members.contains(&new_member_account_id));
	println!("[+] Member {} has been removed successfully.", new_member_account_id.to_ss58check());
	println!(
		"[+] Current members: {:?}",
		members.iter().map(|m| m.to_ss58check()).collect::<Vec<String>>()
	);
}
