//! Shared helpers used by the Criterion bench suite.
//!
//! Keeping this in one module avoids each bench redefining identical
//! constants, touch loops, and competitor setups.

pub mod alloc;
pub mod competitors;
pub mod config;
pub mod mt;
pub mod sizes;
pub mod strategy;
pub mod touch;
