//! The FFXIV side of reading a shader package.
//!
//! A package identifies every resource and parameter it binds by a hash of the name rather than by
//! the name, so [`names`] is what turns one back into the other. Reading the bytecode itself is not
//! game-specific and lives in the `hlsl` crate; what a register is called comes from here.

pub mod names;
