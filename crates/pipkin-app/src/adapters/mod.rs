//! Backend adapters: the built-in engine (real mode) and the deterministic demo adapter
//! (explicit `--demo`, kept for regression tests).

pub mod attach;
pub mod demo;
mod diff;
mod fixtures;
pub mod native;
mod rng;
pub mod script;
pub mod session;
mod story;
pub mod workspace;

#[cfg(test)]
mod tests;

pub use demo::{DemoBackend, DemoOptions};
