//! Chip-independent mining core: double SHA-256, compact targets, Stratum v1
//! job handling, and the boundary a hashboard backend has to implement.
//!
//! Nothing here talks to hardware. Chip backends sit behind [`chip::ChipBackend`]
//! and the only one in this crate is a software reference used by the tests.

#![forbid(unsafe_code)]

pub mod chip;
pub mod hash;
pub mod header;
pub mod hex;
pub mod miner;
pub mod pool;
pub mod stratum;
pub mod target;
