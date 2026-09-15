//! Model collaboration without owning the host's event loop or tool permissions.
pub mod adapter;
#[cfg(feature = "python")]
mod bindings;
pub mod configuration;
pub mod contracts;
pub mod engine;
pub mod events;
mod planning;
pub mod router;
pub mod store;
pub mod strategy;
pub mod tools;
