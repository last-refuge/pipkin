//! The contract between the Pipkin desktop and its built-in engine.
//!
//! Everything here is plain data: serde types, hard limits and a length-prefixed frame codec.
//! The same types are used by the in-process host (direct channels) and by the private worker
//! transport (inherited pipes), so the topology can change without changing the contract.
//!
//! Versioning: [`PROTOCOL_VERSION`] changes whenever a message changes shape incompatibly. The
//! engine refuses a client with a different protocol in [`api::Call::Hello`]; it never guesses.

pub mod api;
pub mod content;
pub mod frame;
pub mod limits;
pub mod view;

pub use api::*;
pub use content::*;
pub use view::*;

/// Wire protocol version. Bump on any incompatible change to [`api`], [`content`] or [`view`].
// Version 3 requires plugin-profile inspection for imports, including explicit serverDir.
pub const PROTOCOL_VERSION: u32 = 3;
