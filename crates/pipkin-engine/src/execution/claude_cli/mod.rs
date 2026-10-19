//! Version-qualified local CLI transport. No credential extraction or native fallback.
//! The caller owns trust, admission and durable settlement; transport is not authority.

pub(crate) mod coding_policy;
pub mod discovery;
pub mod events;
pub(crate) mod orchestration_child;
pub(crate) mod orchestration_control;
pub mod orchestration_helper;
pub(crate) mod orchestration_input;
pub mod orchestration_observer;
pub mod orchestration_policy;
pub mod orchestration_protocol;
pub mod orchestration_wire;
pub mod permission;
pub mod permission_helper;
pub mod permission_wire;
pub mod policy;
pub mod preflight;
pub mod process;
pub mod stream;
pub mod usage_metadata;
pub(crate) mod write_policy;

#[cfg(all(test, unix))]
mod pipeline_tests;
#[cfg(all(test, unix))]
pub(crate) mod test_support;
