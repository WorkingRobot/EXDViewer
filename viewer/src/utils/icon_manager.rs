use std::{collections::HashMap, sync::Arc};

use egui::{
    ColorImage, ImageSource, TextureHandle, TextureOptions, load::SizedTexture, mutex::Mutex,
};
use either::Either;
use image::RgbaImage;
use url::Url;

use super::{
    CloneableResult, ConvertiblePromise, PromiseKind, TrackedPromise,
    cloneable_error::CloneableError,
};

pub enum ManagedIcon {
    Loaded(ImageSource<'static>),
    Failed(CloneableError),
    Loading,
    NotLoaded,
}

type IconPromise = TrackedPromise<anyhow::Result<Either<Url, RgbaImage>>>;

type ConvertibleIconPromise =
    ConvertiblePromise<IconPromise, CloneableResult<ImageSource<'static>>>;

#[derive(Clone, Default)]
pub struct IconManager(Arc<Mutex<IconManagerImpl>>);

#[derive(Default)]
struct IconManagerImpl {
    /// Keyed by the resolved path, so an icon that resolves elsewhere once the locale or the path
    /// index changes is a miss rather than a stale hit.
    cache: HashMap<String, ConvertibleIconPromise>,
    loaded_handles: Vec<TextureHandle>,
}

impl IconManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&self) {
        self.0.lock().clear();
    }

    pub fn get_or_insert_icon(
        &self,
        path: &str,
        context: &egui::Context,
        promise_creator: impl FnOnce() -> IconPromise,
    ) -> ManagedIcon {
        self.0
            .lock()
            .get_or_insert_icon_promise(path, context, promise_creator)
    }
}

impl IconManagerImpl {
    pub fn clear(&mut self) {
        self.loaded_handles.clear();
        self.cache.clear();
    }

    fn convert_promise(
        handles: &mut Vec<TextureHandle>,
        path: &str,
        ctx: &egui::Context,
        result: <IconPromise as PromiseKind>::Output,
    ) -> CloneableResult<ImageSource<'static>> {
        match result {
            Ok(Either::Left(url)) => Ok(ImageSource::Uri(url.to_string().into())),
            Ok(Either::Right(data)) => {
                let handle = ctx.load_texture(
                    path,
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

    pub fn get_or_insert_icon_promise(
        &mut self,
        path: &str,
        context: &egui::Context,
        promise_creator: impl FnOnce() -> IconPromise,
    ) -> ManagedIcon {
        let ret = self
            .cache
            .entry(path.to_owned())
            .or_insert_with(|| ConvertiblePromise::new_promise(promise_creator()))
            .get_mut(|r| Self::convert_promise(&mut self.loaded_handles, path, context, r))
            .cloned();
        match ret {
            Some(Ok(image)) => ManagedIcon::Loaded(image),
            Some(Err(e)) => ManagedIcon::Failed(e),
            None => ManagedIcon::Loading,
        }
    }
}
