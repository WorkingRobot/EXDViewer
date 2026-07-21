//! Decoding and looping playback of FFXIV sound files. Decoding is shared; only the output
//! backend is platform-specific (rodio natively, Web Audio on wasm).

mod decode;
pub use decode::{Decoded, decode};

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(not(target_arch = "wasm32"))]
pub use native::Player;

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
pub use web::Player;
