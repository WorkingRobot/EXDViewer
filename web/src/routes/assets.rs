use std::env::{current_dir, current_exe};

use actix_files::Files;
use actix_web::dev::HttpServiceFactory;

pub fn service() -> impl HttpServiceFactory {
    Files::new(
        "/",
        current_exe()
            .map(|p| p.parent().map(|p| p.to_path_buf()).unwrap_or(p))
            .unwrap_or_else(|_| current_dir().unwrap())
            .join("static"),
    )
    .index_file("index.html")
}
