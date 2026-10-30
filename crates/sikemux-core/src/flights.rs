//! Work that many callers may ask for at once, done once and shared.

use std::collections::HashMap;
use std::future::Future;
use std::hash::Hash;
use std::sync::{Arc, Mutex};

use sikemux_process::sync::Locked;
use tokio::sync::OnceCell;

/// The work under way, by what it answers.
pub(crate) type Flights<K, T> = Mutex<HashMap<K, Arc<OnceCell<T>>>>;

/// What `key` names, worked out once however many ask while it is under way.
pub(crate) async fn shared<K: Hash + Eq + Clone, T: Clone>(
    flights: &Flights<K, T>,
    key: K,
    work: impl Future<Output = T>,
) -> T {
    let flight = flights.locked().entry(key.clone()).or_default().clone();
    let result = flight.get_or_init(|| work).await.clone();
    let mut flights = flights.locked();
    if flights
        .get(&key)
        .is_some_and(|current| Arc::ptr_eq(current, &flight))
    {
        flights.remove(&key);
    }
    result
}
