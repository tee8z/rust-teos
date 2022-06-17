//! The Eye of Satoshi - Lightning watchtower.
//!
//! A watchtower implementation written in Rust.

pub mod protos {
    tonic::include_proto!("teos.v2");
}
pub mod api;
pub mod bitcoin_cli;
pub mod carrier;
pub mod chain_monitor;
pub mod cli_config;
pub mod config;
pub mod dbm;
pub mod startup;

#[doc(hidden)]
mod errors;
mod extended_appointment;
pub mod gatekeeper;
pub mod responder;
#[doc(hidden)]
mod rpc_errors;
pub mod watcher;

#[cfg(test)]
mod test_utils;
