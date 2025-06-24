/*
   Copyright 2019 Supercomputing Systems AG
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

use crate::{
	api::{Api, Error, Result},
	rpc::{HandleSubscription, Request, Subscribe},
	GetChainInfo, GetStorage,
};
use ac_compose_macros::rpc_params;
use ac_node_api::{metadata::Metadata, EventDetails, EventRecord, Events, Phase};
use ac_primitives::config::Config;
#[cfg(not(feature = "sync-api"))]
use alloc::boxed::Box;
use alloc::{vec, vec::Vec};
use codec::{Decode, Encode};
use core::marker::PhantomData;
use log::*;
use serde::de::DeserializeOwned;
use sp_runtime::traits::{Block as BlockHash, Hash as HashTrait};
use sp_storage::StorageChangeSet;

pub type EventSubscriptionFor<Client, Hash> =
	EventSubscription<<Client as Subscribe>::Subscription<StorageChangeSet<Hash>>, Hash>;

#[maybe_async::maybe_async(?Send)]
pub trait FetchEvents {
	type Hash: Encode + Decode;

	/// Fetch all block events from node for the given block hash.
	async fn fetch_events_from_block(&self, block_hash: Self::Hash) -> Result<Events<Self::Hash>>;

	/// Fetch all associated events for a given extrinsic hash and block hash.
	async fn fetch_events_for_extrinsic(
		&self,
		block_hash: Self::Hash,
		extrinsic_hash: Self::Hash,
	) -> Result<Vec<EventDetails<Self::Hash>>>;
}

#[maybe_async::maybe_async(?Send)]
impl<T, Client> FetchEvents for Api<T, Client>
where
	T: Config,
	Client: Request,
{
	type Hash = T::Hash;

	async fn fetch_events_from_block(&self, block_hash: Self::Hash) -> Result<Events<Self::Hash>> {
		let key = crate::storage_key("System", "Events");
		let event_bytes = self
			.get_opaque_storage_by_key(key, Some(block_hash))
			.await?
			.ok_or(Error::BlockNotFound)?;
		let events =
			Events::<Self::Hash>::new(self.metadata().clone(), Default::default(), event_bytes);
		Ok(events)
	}

	async fn fetch_events_for_extrinsic(
		&self,
		block_hash: Self::Hash,
		extrinsic_hash: Self::Hash,
	) -> Result<Vec<EventDetails<Self::Hash>>> {
		let extrinsic_index =
			self.retrieve_extrinsic_index_from_block(block_hash, extrinsic_hash).await?;
		let block_events = self.fetch_events_from_block(block_hash).await?;
		self.filter_extrinsic_events(block_events, extrinsic_index)
	}
}

/// Wrapper around a Event `StorageChangeSet` subscription.
/// Simplifies the event retrieval from the subscription.
pub struct EventSubscription<Subscription, Hash> {
	pub subscription: Subscription,
	pub metadata: Metadata,
	_phantom: PhantomData<Hash>,
}

impl<Subscription, Hash> EventSubscription<Subscription, Hash> {
	/// Create a new wrapper around the subscription.
	pub fn new(subscription: Subscription, metadata: Metadata) -> Self {
		Self { subscription, metadata, _phantom: Default::default() }
	}

	/// Update the metadata.
	pub fn update_metadata(&mut self, metadata: Metadata) {
		self.metadata = metadata
	}
}

impl<Subscription, Hash> EventSubscription<Subscription, Hash>
where
	Hash: DeserializeOwned + Copy + Encode + Decode,
	Subscription: HandleSubscription<StorageChangeSet<Hash>>,
{
	/// Wait for the next value from the internal subscription.
	/// Upon encounter, it retrieves and decodes the expected `EventRecord`.
	#[maybe_async::maybe_async(?Send)]
	pub async fn next_events<RuntimeEvent: Decode, Topic: Decode>(
		&mut self,
	) -> Option<Result<Vec<EventRecord<RuntimeEvent, Topic>>>> {
		let change_set = match self.subscription.next().await? {
			Ok(set) => set,
			Err(e) => return Some(Err(Error::RpcClient(e))),
		};
		// Since we subscribed to only the events key, we can simply take the first value of the
		// changes in the set. Also, we don't care about the key but only the data, so take
		// the second value in the tuple of two.
		let storage_data = change_set.changes[0].1.as_ref()?;
		let event_records = Decode::decode(&mut storage_data.0.as_slice()).map_err(Error::Codec);
		Some(event_records)
	}

	/// Wait for the next value from the internal subscription.
	/// Upon encounter, it retrieves and decodes the expected `EventDetails`.
	//
	// On the contrary to `next_events` this function only needs up-to-date metadata
	// and is therefore updateable during runtime.
	#[maybe_async::maybe_async(?Send)]
	pub async fn next_events_from_metadata(&mut self) -> Option<Result<Events<Hash>>> {
		let change_set = match self.subscription.next().await? {
			Ok(set) => set,
			Err(e) => return Some(Err(Error::RpcClient(e))),
		};
		let block_hash = change_set.block;
		// Since we subscribed to only the events key, we can simply take the first value of the
		// changes in the set. Also, we don't care about the key but only the data, so take
		// the second value in the tuple of two.
		let storage_data = change_set.changes[0].1.as_ref()?;
		let event_bytes = storage_data.0.clone();

		let events = Events::<Hash>::new(self.metadata.clone(), block_hash, event_bytes);
		Some(Ok(events))
	}

	/// Unsubscribe from the internal subscription.
	#[maybe_async::maybe_async(?Send)]
	pub async fn unsubscribe(self) -> Result<()> {
		self.subscription.unsubscribe().await.map_err(|e| e.into())
	}
}

#[maybe_async::maybe_async(?Send)]
pub trait SubscribeEvents {
	type Client: Subscribe;
	type Hash: DeserializeOwned;

	/// Subscribe to events.
	async fn subscribe_events(&self) -> Result<EventSubscriptionFor<Self::Client, Self::Hash>>;
}

#[maybe_async::maybe_async(?Send)]
impl<T, Client> SubscribeEvents for Api<T, Client>
where
	T: Config,
	Client: Subscribe,
{
	type Client = Client;
	type Hash = T::Hash;

	async fn subscribe_events(&self) -> Result<EventSubscriptionFor<Self::Client, Self::Hash>> {
		let key = crate::storage_key("System", "Events");
		let subscription = self
			.client()
			.subscribe("state_subscribeStorage", rpc_params![vec![key]], "state_unsubscribeStorage")
			.await
			.map(|sub| EventSubscription::new(sub, self.metadata().clone()))?;
		Ok(subscription)
	}
}

impl<T, Client> Api<T, Client>
where
	T: Config,
	Client: Request,
{
	/// Retrieve block details from node and search for the position of the given extrinsic.
	#[maybe_async::maybe_async(?Send)]
	async fn retrieve_extrinsic_index_from_block(
		&self,
		block_hash: T::Hash,
		extrinsic_hash: T::Hash,
	) -> Result<u32> {
		let block = self.get_block(Some(block_hash)).await?.ok_or(Error::BlockNotFound)?;
		let xt_index = block
			.extrinsics()
			.iter()
			.position(|xt| {
				let xt_hash = T::Hasher::hash_of(&xt);
				trace!("Looking for: {:?}, got xt_hash {:?}", extrinsic_hash, xt_hash);
				extrinsic_hash == xt_hash
			})
			.ok_or(Error::ExtrinsicNotFound)?;
		Ok(xt_index as u32)
	}

	/// Filter events and return the ones associated to the given extrinsic index.
	fn filter_extrinsic_events(
		&self,
		events: Events<T::Hash>,
		extrinsic_index: u32,
	) -> Result<Vec<EventDetails<T::Hash>>> {
		let extrinsic_event_results = events.iter().filter(|ev| {
			ev.as_ref()
				.map_or(true, |ev| ev.phase() == Phase::ApplyExtrinsic(extrinsic_index))
		});
		let mut extrinsic_events = Vec::new();
		for event_details in extrinsic_event_results {
			let event_details = event_details?;
			debug!(
				"associated event_details {:?} {:?}",
				event_details.pallet_name(),
				event_details.variant_name()
			);
			extrinsic_events.push(event_details);
		}
		Ok(extrinsic_events)
	}
}

/*
#[cfg(test)]
mod tests {
	use super::*;
	use crate::{
		api::rpc_api::{
			events::{EventDetails, Events},
			frame_system::System,
			pallet_balances::Balances,
		},
		rpc::mocks::RpcClientMock,
		Error,
	};
	use ac_primitives::{FrameSystemConfig, RococoRuntimeConfig};
	use frame_metadata::{RuntimeMetadataPrefixed, StorageEntryModifier};
	// todo: fix tests
	// use resonance_runtime::{BalancesCall, RuntimeCall, UncheckedExtrinsic};
	use sp_core::{
		crypto::{Pair, Ss58Codec},
		sr25519, H256,
	};
	use std::{
		collections::{HashMap, HashSet},
		fs,
	};

	fn default_header() -> sp_runtime::generic::Header<u32, sp_runtime::traits::Blake2_256> {
		sp_runtime::generic::Header {
			parent_hash: H256::from_slice(&[1; 32]),
			number: Default::default(),
			state_root: H256::from_slice(&[2; 32]),
			extrinsics_root: H256::from_slice(&[3; 32]),
			digest: Default::default(),
		}
	}
}
*/
