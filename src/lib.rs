//! qvm - thin CLI for managing KVM/libvirt VMs.
//!
//! Most logic is in the binary. This library surface exists so the
//! integration test suite can exercise individual modules directly
//! without touching libvirt or the network.

pub mod cloudinit;
pub mod cmd;
pub mod commands;
pub mod config;
pub mod error;
pub mod libvirt;
pub mod tui;
pub mod util;
