//! Assets a viewer needs beyond the file it was handed.
//!
//! A material names textures, a model names materials, and a `.uld` names both. Rendering any of
//! them means fetching more files after the first decode has already happened, so a viewer asks for
//! a path and gets whatever is ready this frame, with the fetch continuing in the background.
//!
//! Entries are kept in an LRU keyed by path, the same shape the sheet cache uses, so moving between
//! materials that share a texture does not refetch it.

use std::num::NonZeroUsize;

use egui::TextureHandle;
use lru::LruCache;

use crate::backend::Backend;
use crate::utils::TrackedPromise;

use super::Load;

/// Decoded textures held for viewers. Sized for a handful of materials' worth of samplers rather
/// than a browsing session's, since each entry is an uploaded GPU texture.
const CAPACITY: usize = 32;

/// Longest edge a dependency preview is drawn at. Textures are decoded from the smallest mipmap
/// that still covers it rather than from full size.
pub const THUMBNAIL_SIZE: u16 = 128;

/// What a viewer gets back when it asks for a dependency.
pub enum Dep<'a> {
    /// Still being fetched or decoded; the viewer should leave room for it.
    Pending,
    Ready(&'a TextureHandle),
    /// Fetched but unusable.
    Failed,
}

pub struct Deps {
    textures: LruCache<String, Load<Vec<u8>, Option<TextureHandle>>>,
}

impl Default for Deps {
    fn default() -> Self {
        Self {
            textures: LruCache::new(NonZeroUsize::new(CAPACITY).unwrap()),
        }
    }
}

impl Deps {
    /// Ask for a texture by path, starting a fetch if this is the first time it has been requested.
    ///
    /// Called during rendering, so it never blocks: the first call returns [`Dep::Pending`] and
    /// starts the work, and later frames return the result once it lands.
    pub fn texture(&mut self, ctx: &egui::Context, backend: &Backend, path: &str) -> Dep<'_> {
        if self.textures.peek(path).is_none() {
            let files = backend.files().clone();
            let wanted = path.to_string();
            self.textures.put(
                path.to_string(),
                Load::Loading(TrackedPromise::spawn_local(async move {
                    files.read(&wanted).await
                })),
            );
        }

        // Promote on use so the entries a viewer is actively drawing are the last to be evicted.
        let entry = self.textures.get_mut(path).expect("just inserted");
        if let Load::Loading(promise) = entry
            && let Some(result) = promise.try_get()
        {
            let decoded = match result {
                Ok(bytes) => decode(ctx, path, bytes),
                Err(error) => Err(error.to_string()),
            };
            *entry = Load::Ready(
                decoded
                    .inspect_err(|error| log::error!("assets/deps: {path}: {error}"))
                    .ok(),
            );
        }

        match entry {
            Load::Idle | Load::Loading(_) => Dep::Pending,
            Load::Ready(Some(texture)) => Dep::Ready(texture),
            Load::Ready(None) | Load::Failed(_) => Dep::Failed,
        }
    }
}

fn decode(ctx: &egui::Context, path: &str, bytes: &[u8]) -> Result<TextureHandle, String> {
    let texture = crate::utils::tex_loader::decode_preview(bytes, path, THUMBNAIL_SIZE)
        .map_err(|e| e.to_string())?;
    let image = texture.to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    Ok(ctx.load_texture(
        format!("dep:{path}"),
        egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw()),
        egui::TextureOptions::LINEAR,
    ))
}
