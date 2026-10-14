//! External-agent execution primitives. These are not provider adapters.
//! No runtime RPC enables a CLI executor until admission, supervision and capability gates
//! are integrated; creating these helpers does not authorize an invocation.

pub mod claude_cli;
