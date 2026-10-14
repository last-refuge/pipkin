//! Version-qualified local CLI transport. No credential extraction or native fallback.
//! The caller owns trust, admission and durable settlement; transport is not authority.

pub mod events;
pub mod policy;
pub mod preflight;
pub mod process;
pub mod stream;

#[cfg(all(test, unix))]
mod pipeline_tests;
