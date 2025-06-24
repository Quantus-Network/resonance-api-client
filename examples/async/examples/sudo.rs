//! This example shows how to use the compose_extrinsic macro to create a sudo extrinsic
//! that is signed with a dilithium key.

use codec::Compact;
use dilithium_crypto::pair::{crystal_alice, dilithium_bob};
use sp_runtime::traits::IdentifyAccount;
use substrate_api_client::{
	ac_compose_macros::{compose_call, compose_extrinsic},
	ac_primitives::{
		resonance_runtime_config::ResonanceRuntimeConfig, Config,
		ExtrinsicSigner as GenericExtrinsicSigner, SignExtrinsic, UncheckedExtrinsic,
	},
	rpc::JsonrpseeClient,
	Api, GetAccountInformation, SubmitAndWatch, XtStatus,
};

// Define an extrinsic signer type which sets the generic types of the `GenericExtrinsicSigner`.
type ExtrinsicSigner = GenericExtrinsicSigner<ResonanceRuntimeConfig>;

// To access the ExtrinsicAddress type of the Signer, we need to do this via the trait `SignExtrinsic`.
type ExtrinsicAddressOf<Signer> = <Signer as SignExtrinsic<AccountId>>::ExtrinsicAddress;

// AccountId type of the runtime.
type AccountId = <ResonanceRuntimeConfig as Config>::AccountId;
type Address = <ResonanceRuntimeConfig as Config>::Address;
type Balance = <ResonanceRuntimeConfig as Config>::Balance;

#[tokio::main]
async fn main() {
	env_logger::init();

	// Initialize api and set the signer (sender) that is used to sign the extrinsics.
	// The sudoer is set to `crystal_alice`.
	let sudoer_signer = crystal_alice();
	let client = JsonrpseeClient::with_default_url().await.unwrap();
	let mut api = Api::<ResonanceRuntimeConfig, _>::new(client).await.unwrap();
	api.set_signer(ExtrinsicSigner::new(sudoer_signer));

	// Set the recipient of newly issued funds.
	let recipient = dilithium_bob().into_account();

	// Get the current balance of the recipient.
	let recipient_balance = api
		.get_account_data(&recipient)
		.await
		.unwrap()
		.map(|data| data.free)
		.unwrap_or_default();
	println!("[+] Recipient's Free Balance is now {}\n", recipient_balance);

	// Compose a call that should only be executable via Sudo.
	let recipients_extrinsic_address: ExtrinsicAddressOf<ExtrinsicSigner> =
		recipient.clone().into();
	let new_balance: Balance = 2_000_000_000_000_000;
	let call: ([u8; 2], Address, Compact<Balance>) = compose_call!(
		api.metadata(),
		"Balances",
		"force_set_balance",
		recipients_extrinsic_address,
		Compact(new_balance)
	)
	.unwrap();

	let xt: UncheckedExtrinsic<_, _, _, _> =
		compose_extrinsic!(&api, "Sudo", "sudo", call).unwrap();

	// Send and watch extrinsic until in block.
	let block_hash = api
		.submit_and_watch_extrinsic_until(xt, XtStatus::InBlock)
		.await
		.unwrap()
		.block_hash;
	println!("[+] Extrinsic got included. Block Hash: {:?}", block_hash);

	// Ensure the extrinsic has been executed.
	let recipient_new_balance = api
		.get_account_data(&recipient)
		.await
		.unwrap()
		.map(|data| data.free)
		.unwrap_or_default();
	assert_eq!(recipient_new_balance, new_balance);
	println!("[+] Recipient's new balance is now {}\n", recipient_new_balance);
}
