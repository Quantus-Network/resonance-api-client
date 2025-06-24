//! This example demonstrates how to make a balance transfer between accounts.

use dilithium_crypto::pair::{crystal_alice, dilithium_bob};
use sp_runtime::traits::IdentifyAccount;
use substrate_api_client::{
	ac_primitives::{
		resonance_runtime_config::ResonanceRuntimeConfig, Config,
		ExtrinsicSigner as GenericExtrinsicSigner,
	},
	extrinsic::BalancesExtrinsics,
	rpc::JsonrpseeClient,
	Api, GetAccountInformation, SubmitAndWatch, XtStatus,
};

// Define an extrinsic signer type.
type ExtrinsicSigner = GenericExtrinsicSigner<ResonanceRuntimeConfig>;

// AccountId type of the runtime.
type AccountId = <ResonanceRuntimeConfig as Config>::AccountId;

#[tokio::main]
async fn main() {
	env_logger::init();

	// Initialize api and set the signer.
	let signer = crystal_alice();
	let client = JsonrpseeClient::with_default_url().await.unwrap();
	let mut api = Api::<ResonanceRuntimeConfig, _>::new(client).await.unwrap();
	api.set_signer(ExtrinsicSigner::new(signer));

	// Define recipient and amount to transfer.
	let recipient: AccountId = dilithium_bob().into_account();
	let amount_to_transfer = 1_000_000_000_000;

	// Get initial balances.
	let sender = crystal_alice().into_account();
	let initial_sender_balance = api
		.get_account_data(&sender)
		.await
		.unwrap()
		.map(|data| data.free)
		.unwrap_or_default();
	let initial_recipient_balance = api
		.get_account_data(&recipient)
		.await
		.unwrap()
		.map(|data| data.free)
		.unwrap_or_default();

	println!("[+] Sender's initial balance: {}", initial_sender_balance);
	println!("[+] Recipient's initial balance: {}\n", initial_recipient_balance);

	// Create and submit the transfer extrinsic.
	let xt = api
		.balance_transfer_allow_death(recipient.clone().into(), amount_to_transfer)
		.await
		.unwrap();
	let report = api.submit_and_watch_extrinsic_until(xt, XtStatus::Finalized).await.unwrap();

	println!("[+] Transfer extrinsic got finalized in block {:?}", report.block_hash.unwrap());

	// Get final balances.
	let final_sender_balance = api
		.get_account_data(&sender)
		.await
		.unwrap()
		.map(|data| data.free)
		.unwrap_or_default();
	let final_recipient_balance = api
		.get_account_data(&recipient)
		.await
		.unwrap()
		.map(|data| data.free)
		.unwrap_or_default();

	println!("[+] Sender's final balance: {}", final_sender_balance);
	println!("[+] Recipient's final balance: {}", final_recipient_balance);

	// Verify the balance changes.
	assert!(final_sender_balance < initial_sender_balance - amount_to_transfer);
	assert_eq!(final_recipient_balance, initial_recipient_balance + amount_to_transfer);

	println!("\n[+] Transfer successful");
}
