use std::env::{current_dir, current_exe};

use actix_files::Files;
use actix_web::{dev::HttpServiceFactory, get};

pub fn service() -> impl HttpServiceFactory {
    (
        index,
        Files::new(
            "/",
            current_exe()
                .map(|p| p.parent().map(|p| p.to_path_buf()).unwrap_or(p))
                .unwrap_or_else(|_| current_dir().unwrap())
                .join("static"),
        )
        .index_file("index.html"),
    )
}

#[get("/{tail:[^\\.]+}")]
async fn index(path: web::Data<PathBuf>) -> actix_web::Result<NamedFile> {
    Ok(NamedFile::open(path.join("index.html"))?)
}
