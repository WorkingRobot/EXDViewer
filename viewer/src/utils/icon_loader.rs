use std::{collections::HashMap, io::Cursor, sync::Arc};

use egui::{
    ColorImage, SizeHint,
    load::{BytesPoll, ImageLoadResult, ImageLoader, ImagePoll, LoadError},
    mutex::Mutex,
};

pub fn install_tex_loader(ctx: &egui::Context) {
    if !ctx.is_loader_installed(TexLoader::ID) {
        ctx.add_image_loader(Arc::new(TexLoader::default()));
        log::trace!("installed TexLoader");
    }
}

fn is_supported(uri: &str) -> bool {
    uri.ends_with(".tex")
}

#[derive(Clone)]
struct Tex(Arc<ColorImage>);

impl Tex {
    fn load(data: &[u8], uri: &str) -> Result<Self, String> {
        use ironworks::file::{File as _, tex};
        let texture =
            tex::Texture::read(Cursor::new(data.to_vec())).map_err(|why| why.to_string())?;
        let image = crate::utils::tex_loader::decode_mip(&texture, 0, uri)
            .map_err(|why| why.to_string())?;
        let rgba = image.to_rgba8();
        let size = [rgba.width() as usize, rgba.height() as usize];
        let image =
            egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_flat_samples().as_slice());
        Ok(Self(Arc::new(image)))
    }

    pub fn byte_len(&self) -> usize {
        self.0.pixels.len() * size_of::<egui::Color32>()
    }
}

type Entry = Result<Tex, String>;

#[derive(Default)]
pub struct TexLoader {
    cache: Mutex<HashMap<String, Entry>>,
}

impl TexLoader {
    pub const ID: &'static str = egui::generate_loader_id!(TexLoader);
}

impl ImageLoader for TexLoader {
    fn id(&self) -> &str {
        Self::ID
    }

    fn load(&self, ctx: &egui::Context, uri: &str, _: SizeHint) -> ImageLoadResult {
        if !is_supported(uri) {
            return Err(LoadError::NotSupported);
        }

        let mut cache = self.cache.lock();
        if let Some(entry) = cache.get(uri).cloned() {
            match entry {
                Ok(image) => Ok(ImagePoll::Ready {
                    image: image.0.clone(),
                }),
                Err(error) => Err(LoadError::Loading(error)),
            }
        } else {
            match ctx.try_load_bytes(uri) {
                Ok(BytesPoll::Ready { bytes, .. }) => {
                    log::trace!("started loading {uri:?}");

                    let result = Tex::load(&bytes, uri);

                    log::trace!("finished loading {uri:?}");

                    cache.insert(uri.into(), result.clone());

                    match result {
                        Ok(image) => Ok(ImagePoll::Ready {
                            image: image.0.clone(),
                        }),
                        Err(error) => Err(LoadError::Loading(error)),
                    }
                }
                Ok(BytesPoll::Pending { size }) => Ok(ImagePoll::Pending { size }),
                Err(error) => Err(error),
            }
        }
    }

    fn forget(&self, uri: &str) {
        let _ = self.cache.lock().remove(uri);
    }

    fn forget_all(&self) {
        self.cache.lock().clear();
    }

    fn byte_size(&self) -> usize {
        self.cache
            .lock()
            .values()
            .map(|entry| match entry {
                Ok(entry_value) => entry_value.byte_len(),
                Err(error) => error.len(),
            })
            .sum()
    }
}
