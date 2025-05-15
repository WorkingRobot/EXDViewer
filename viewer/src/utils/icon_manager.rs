use std::{collections::HashMap, sync::Arc};

use egui::{
    ColorImage, ImageSource, TextureHandle, TextureOptions, load::SizedTexture, mutex::Mutex,
};
use either::Either;
use image::RgbaImage;
use url::Url;

use super::{CloneableResult, ConvertiblePromise, TrackedPromise, cloneable_error::CloneableError};

pub enum ManagedIcon {
    Loaded(ImageSource<'static>),
    Failed(CloneableError),
    Loading,
    NotLoaded,
}

#[derive(Clone, Default)]
pub struct IconManager(Arc<Mutex<IconManagerImpl>>);

#[derive(Default)]
struct IconManagerImpl {
    cache: HashMap<
        (u32, bool),
        ConvertiblePromise<
            TrackedPromise<anyhow::Result<Either<Url, RgbaImage>>>,
            CloneableResult<ImageSource<'static>>,
        >,
    >,
    loaded_handles: Vec<TextureHandle>,
}

impl IconManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&self) {
        self.0.lock().clear()
    }

    // None = not loaded, Some(None) = loaded but failed/doesn't exist, Some(Some) = loaded successfully
    // pub fn get_icon(&self, icon_id: u32, hires: bool, context: &egui::Context) -> ManagedIcon {
    //     self.0.lock().get_icon(icon_id, hires, context)
    // }

    pub fn get_or_insert_icon(
        &self,
        icon_id: u32,
        hires: bool,
        context: &egui::Context,
        promise_creator: impl FnOnce() -> TrackedPromise<anyhow::Result<Either<Url, RgbaImage>>>,
    ) -> ManagedIcon {
        self.0
            .lock()
            .get_or_insert_icon_promise(icon_id, hires, context, promise_creator)
    }
}

impl IconManagerImpl {
    pub fn clear(&mut self) {
        self.loaded_handles.clear();
        self.cache.clear();
    }

    fn convert_promise(
        handles: &mut Vec<TextureHandle>,
        icon_id: u32,
        hires: bool,
        ctx: &egui::Context,
        result: anyhow::Result<Either<Url, RgbaImage>>,
    ) -> CloneableResult<ImageSource<'static>> {
        match result {
            Ok(Either::Left(url)) => Ok(ImageSource::Uri(url.to_string().into())),
            Ok(Either::Right(data)) => {
                let handle = ctx.load_texture(
                    format!("Icon {icon_id}{}", if hires { " (hr1)" } else { "" }),
                    ColorImage::from_rgba_unmultiplied(
                        [data.width() as _, data.height() as _],
                        data.as_flat_samples().as_slice(),
                    ),
                    TextureOptions::LINEAR,
                );
                let ret = SizedTexture::from_handle(&handle);
                handles.push(handle);
                Ok(ImageSource::Texture(ret))
            }
            Err(e) => {
                log::error!("Failed to load icon: {e:?}");
                Err(e.into())
            }
        }
    }

    // pub fn get_icon(&mut self, icon_id: u32, hires: bool, context: &egui::Context) -> ManagedIcon {
    //     let entry = match self.cache.get_mut(&(icon_id, hires)) {
    //         Some(entry) => entry,
    //         None => return ManagedIcon::NotLoaded,
    //     };
    //     let ret = entry
    //         .get(|r| Self::convert_promise(&mut self.loaded_handles, icon_id, hires, context, r))
    //         .cloned();
    //     match ret {
    //         Some(Ok(image)) => ManagedIcon::Loaded(image),
    //         Some(Err(e)) => ManagedIcon::Failed(e),
    //         None => ManagedIcon::Loading,
    //     }
    // }

    pub fn get_or_insert_icon_promise(
        &mut self,
        icon_id: u32,
        hires: bool,
        context: &egui::Context,
        promise_creator: impl FnOnce() -> TrackedPromise<anyhow::Result<Either<Url, RgbaImage>>>,
    ) -> ManagedIcon {
        let ret = self
            .cache
            .entry((icon_id, hires))
            .or_insert_with(|| ConvertiblePromise::new_promise(promise_creator()))
            .get(|r| Self::convert_promise(&mut self.loaded_handles, icon_id, hires, context, r))
            .cloned();
        match ret {
            Some(Ok(image)) => ManagedIcon::Loaded(image),
            Some(Err(e)) => ManagedIcon::Failed(e),
            None => ManagedIcon::Loading,
        }
    }
}
