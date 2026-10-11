//! Pipkin's built-in agent engine.
//!
//! GPUI-free and transport-free: hosts (the in-process harness, the private worker process)
//! drive it through [`pipkin_engine_protocol`] calls and receive committed events.

#![allow(clippy::too_many_arguments)]

pub mod auth;
pub mod config;
pub mod context;
pub mod import;
pub mod provider;
pub mod store;
pub mod tools;
pub mod util;
