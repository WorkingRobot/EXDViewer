use std::{collections::HashMap, sync::Arc};

use egui::{
    ColorImage, ImageSource, TextureHandle, TextureOptions, load::SizedTexture, mutex::RwLock,
};
use image::RgbaImage;
use url::Url;

#[derive(Clone, Default)]
pub struct IconManager(Arc<RwLock<IconManagerImpl>>);

#[derive(Default)]
struct IconManagerImpl {
    cache: HashMap<u32, ImageSource<'static>>,
    loaded_handles: Vec<TextureHandle>,
}

impl IconManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&self) {
        self.0.write().clear()
    }

    pub fn insert_icon_image_source(
        &self,
        icon_id: u32,
        data: ImageSource<'static>,
    ) -> ImageSource<'static> {
        self.0.write().insert_icon_image_source(icon_id, data)
    }

    pub fn insert_icon_url(&self, icon_id: u32, data: Url) -> ImageSource<'static> {
        self.0.write().insert_icon_url(icon_id, data)
    }

    pub fn insert_icon_texture(
        &self,
        icon_id: u32,
        ctx: &egui::Context,
        data: RgbaImage,
    ) -> ImageSource<'static> {
        self.0.write().insert_icon_texture(icon_id, ctx, data)
    }

    pub fn get_icon(&self, icon_id: u32) -> Option<ImageSource<'static>> {
        self.0.read().get_icon(icon_id)
    }
}

impl IconManagerImpl {
    pub fn clear(&mut self) {
        self.loaded_handles.clear();
        self.cache.clear();
    }

    pub fn insert_icon_image_source(
        &mut self,
        icon_id: u32,
        data: ImageSource<'static>,
    ) -> ImageSource<'static> {
        self.cache.entry(icon_id).insert_entry(data).get().clone()
    }

    pub fn insert_icon_url(&mut self, icon_id: u32, data: Url) -> ImageSource<'static> {
        self.insert_icon_image_source(icon_id, ImageSource::Uri(data.to_string().into()))
    }

    pub fn insert_icon_texture(
        &mut self,
        icon_id: u32,
        ctx: &egui::Context,
        data: RgbaImage,
    ) -> ImageSource<'static> {
        let handle = ctx.load_texture(
            format!("Icon {icon_id}"),
            ColorImage::from_rgba_unmultiplied(
                [data.width() as _, data.height() as _],
                data.as_flat_samples().as_slice(),
            ),
            TextureOptions::LINEAR,
        );
        let ret = self.insert_icon_image_source(
            icon_id,
            ImageSource::Texture(SizedTexture::from_handle(&handle)),
        );
        self.loaded_handles.push(handle);
        ret
    }

    pub fn get_icon(&self, icon_id: u32) -> Option<ImageSource<'static>> {
        self.cache.get(&icon_id).cloned()
    }
}
