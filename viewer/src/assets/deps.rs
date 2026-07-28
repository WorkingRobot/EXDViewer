//! Assets a viewer needs beyond the file it was handed.
//!
//! A material names textures, a model names materials, and a `.uld` names both plus the sheet rows
//! its text comes from. Rendering any of them means fetching more files after the first decode has
//! already happened, so a viewer asks for a path and gets whatever is ready this frame, with the
//! fetch continuing in the background.
//!
//! Entries are kept in an LRU keyed by path, the same shape the sheet cache uses, so moving between
//! materials that share a texture does not refetch it.

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use egui::TextureHandle;
use ironworks::excel::Language;
use ironworks::file::exh::ColumnKind;
use lru::LruCache;

use crate::backend::Backend;
use crate::data::DecodedTexture;
use crate::excel::base::CachedProvider;
use crate::excel::provider::{ExcelHeader, ExcelProvider, ExcelSheet};
use crate::settings::LANGUAGE;
use crate::utils::TrackedPromise;

use super::Load;

/// Entries held per cache.
const CAPACITY: usize = 128;

/// Longest edge a dependency preview is drawn at. Textures are decoded from the smallest mipmap
/// that still covers it rather than from full size.
pub const THUMBNAIL_SIZE: u16 = 128;

/// Longest edge an atlas is decoded at. Part rectangles are fractions of the whole texture, so a
/// reduced mipmap still crops to the right sprite, just a softer one.
const ATLAS_SIZE: u16 = 512;

/// What a viewer gets back when it asks for a dependency.
pub enum Dep<T> {
    /// Still being fetched or decoded; the viewer should leave room for it.
    Pending,
    Ready(T),
    /// Fetched but unusable.
    Failed,
}

/// A texture something cuts sprites out of.
pub struct Atlas {
    texture: TextureHandle,
    size: [u16; 2],
}

impl Atlas {
    pub fn texture(&self) -> &TextureHandle {
        &self.texture
    }

    /// The sprite occupying `rect` of the full-resolution texture, as a uv rectangle. Part
    /// rectangles are stated at full resolution whatever mipmap was decoded, so they are converted
    /// against the source size rather than the handle's.
    pub fn uv(&self, x: u16, y: u16, width: u16, height: u16) -> egui::Rect {
        let (w, h) = (
            f32::from(self.size[0].max(1)),
            f32::from(self.size[1].max(1)),
        );
        let (x, y) = (f32::from(x), f32::from(y));
        egui::Rect::from_min_max(
            egui::pos2(x / w, y / h),
            egui::pos2((x + f32::from(width)) / w, (y + f32::from(height)) / h),
        )
    }
}

/// One sheet's text, held behind an `Arc` so a lookup can be handed out without copying the map.
type Strings = Arc<HashMap<u32, String>>;

pub struct Deps {
    textures: LruCache<String, Load<DecodedTexture, Option<TextureHandle>>>,
    atlases: LruCache<String, Load<DecodedTexture, Option<Atlas>>>,
    strings: HashMap<(String, Language), Load<Strings>>,
}

impl Default for Deps {
    fn default() -> Self {
        Self {
            textures: LruCache::new(NonZeroUsize::new(CAPACITY).unwrap()),
            atlases: LruCache::new(NonZeroUsize::new(CAPACITY).unwrap()),
            strings: HashMap::new(),
        }
    }
}

impl Deps {
    /// Ask for a thumbnail by path, starting a fetch if this is the first time it has been
    /// requested.
    ///
    /// Called during rendering, so it never blocks: the first call returns [`Dep::Pending`] and
    /// starts the work, and later frames return the result once it lands.
    pub fn texture(
        &mut self,
        ctx: &egui::Context,
        backend: &Backend,
        path: &str,
    ) -> Dep<&TextureHandle> {
        poll(
            &mut self.textures,
            ctx,
            backend,
            path,
            THUMBNAIL_SIZE,
            upload,
        )
    }

    /// Ask for an atlas by path, at the resolution sprites are cropped from.
    pub fn atlas(&mut self, ctx: &egui::Context, backend: &Backend, path: &str) -> Dep<&Atlas> {
        poll(
            &mut self.atlases,
            ctx,
            backend,
            path,
            ATLAS_SIZE,
            |ctx, path, decoded| Atlas {
                texture: upload(ctx, path, decoded),
                size: decoded.source,
            },
        )
    }

    /// Ask for a string a file names by row rather than carrying itself, reading the sheet on the
    /// first ask. `None` while that read is in flight, and for a row the sheet has no text for.
    pub fn text(
        &mut self,
        ctx: &egui::Context,
        backend: &Backend,
        sheet: &str,
        row: u32,
    ) -> Option<&str> {
        let language = LANGUAGE.get(ctx);
        let key = (sheet.to_owned(), language);
        if !self.strings.contains_key(&key) {
            let excel = backend.excel().clone();
            let name = sheet.to_owned();
            self.strings.insert(
                key.clone(),
                Load::Loading(TrackedPromise::spawn_local(async move {
                    read_strings(excel, name, language).await
                })),
            );
        }

        let entry = self.strings.get_mut(&key).expect("just inserted");
        if let Load::Loading(promise) = entry
            && let Some(result) = promise.try_get()
        {
            *entry = match result {
                Ok(strings) => Load::Ready(strings.clone()),
                Err(error) => {
                    log::error!("assets/deps: {sheet}: {error}");
                    Load::Failed(error.to_string())
                }
            };
        }

        match entry {
            Load::Ready(strings) => strings.get(&row).map(String::as_str),
            _ => None,
        }
    }
}

/// Read a sheet's text into a map from row id, dropping the rows holding nothing.
///
/// The text is the first string written into a row, which is not always the first column.
async fn read_strings(excel: CachedProvider, name: String, language: Language) -> Result<Strings> {
    let sheet = excel.get_sheet(&name, language).await?;
    let offset = sheet
        .columns()
        .iter()
        .filter(|column| column.kind() == ColumnKind::String)
        .map(|column| u32::from(column.offset()))
        .min()
        .ok_or_else(|| anyhow!("sheet {name} holds no text"))?;

    let mut strings = HashMap::new();
    for row_id in sheet.get_row_ids() {
        let Ok(row) = sheet.get_row(row_id) else {
            continue;
        };
        let Ok(cell) = row.read_string(offset) else {
            continue;
        };
        let text = cell.format().to_string();
        if !text.is_empty() {
            strings.insert(row_id, text);
        }
    }
    Ok(Arc::new(strings))
}

/// Drive one cache entry: start the read on first ask, upload it once it lands, and hand back the
/// value when there is one.
fn poll<'a, T>(
    cache: &'a mut LruCache<String, Load<DecodedTexture, Option<T>>>,
    ctx: &egui::Context,
    backend: &Backend,
    path: &str,
    max_dim: u16,
    build: impl FnOnce(&egui::Context, &str, &DecodedTexture) -> T,
) -> Dep<&'a T> {
    if cache.peek(path).is_none() {
        let files = backend.files().clone();
        let wanted = path.to_string();
        cache.put(
            path.to_string(),
            Load::Loading(TrackedPromise::spawn_local(async move {
                files.read_texture(&wanted, Some(max_dim)).await
            })),
        );
    }

    // Promote on use so the entries a viewer is actively drawing are the last to be evicted.
    let entry = cache.get_mut(path).expect("just inserted");
    if let Load::Loading(promise) = entry
        && let Some(result) = promise.try_get()
    {
        *entry = Load::Ready(match result {
            Ok(decoded) => Some(build(ctx, path, decoded)),
            Err(error) => {
                log::error!("assets/deps: {path}: {error}");
                None
            }
        });
    }

    match entry {
        Load::Idle | Load::Loading(_) => Dep::Pending,
        Load::Ready(Some(value)) => Dep::Ready(value),
        Load::Ready(None) | Load::Failed(_) => Dep::Failed,
    }
}

/// Hand decoded pixels to the renderer. The debug label carries the decoded size as well as the
/// path, since the same texture is held at one size for a thumbnail and another for cropping.
fn upload(ctx: &egui::Context, path: &str, decoded: &DecodedTexture) -> TextureHandle {
    let dimensions = [
        decoded.image.width() as usize,
        decoded.image.height() as usize,
    ];
    ctx.load_texture(
        format!("dep:{}x{}:{path}", dimensions[0], dimensions[1]),
        egui::ColorImage::from_rgba_unmultiplied(dimensions, decoded.image.as_raw()),
        egui::TextureOptions::LINEAR,
    )
}
