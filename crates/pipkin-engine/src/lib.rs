//! Pipkin's built-in agent engine.
//!
//! GPUI-free runtime: hosts (in-process, private worker pipes, and an explicit local endpoint)
//! drive the same [`pipkin_engine_protocol`] calls and receive committed events.

#![allow(clippy::too_many_arguments)]

pub mod auth;
pub mod config;
pub mod context;
pub mod endpoint;
pub mod extensions;
pub mod goals;
pub mod host;
pub mod import;
pub mod plans;
pub mod profiles;
pub mod provider;
pub mod runtime;
pub mod store;
pub mod tools;
pub mod util;
pub mod web;

pub mod delivery;
