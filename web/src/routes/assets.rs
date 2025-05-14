use std::{
    env::{current_dir, current_exe},
    path::PathBuf,
    sync::LazyLock,
};

use actix_files::{Files, NamedFile};
use actix_web::{dev::HttpServiceFactory, get};

static SERVICE_DIRECTORY: LazyLock<PathBuf> = LazyLock::new(|| {
    current_exe()
        .map(|p| p.parent().map(|p| p.to_path_buf()).unwrap_or(p))
        .unwrap_or_else(|_| current_dir().unwrap())
        .join("static")
});

pub fn service() -> impl HttpServiceFactory {
    (
        index,
        Files::new("/", SERVICE_DIRECTORY.clone()).index_file("index.html"),
    )
}

#[get("/{tail:[^\\.]+}")]
async fn index() -> actix_web::Result<NamedFile> {
    Ok(NamedFile::open(SERVICE_DIRECTORY.join("index.html"))?)
}
