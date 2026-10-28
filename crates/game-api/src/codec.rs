//! Moves and settings as they travel from a player's screen to their room,
//! which may be on another server: compact bytes only the game reads.

use serde::Serialize;
use serde::de::DeserializeOwned;

pub fn encode<T: Serialize>(value: &T) -> Vec<u8> {
    postcard::to_allocvec(value).expect("moves and settings always serialize")
}

/// `None` for bytes that aren't a `T`, such as another game's.
pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Option<T> {
    postcard::from_bytes(bytes).ok()
}
