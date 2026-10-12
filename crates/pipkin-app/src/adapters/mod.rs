//! Backend adapters: the Pi engine adapter (real mode), the built-in engine adapter
//! (`--engine native`), and the deterministic demo adapter
//! (explicit `--demo`, kept for regression tests).

pub mod attach;
pub mod demo;
mod diff;
mod fixtures;
pub mod native;
pub mod pi;
mod rng;
pub mod script;
pub mod session;
mod story;
pub mod workspace;

#[cfg(test)]
mod tests;

pub use demo::{DemoBackend, DemoOptions};
