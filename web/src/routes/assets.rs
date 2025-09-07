use std::{
    env::{current_dir, current_exe},
    path::PathBuf,
    sync::LazyLock,
};

use actix_files::{Files, NamedFile};
use actix_web::{
    HttpResponse,
    dev::{HttpServiceFactory, ServiceRequest, ServiceResponse, fn_service},
};

static SERVICE_DIRECTORY: LazyLock<PathBuf> = LazyLock::new(|| {
    current_exe()
        .map(|p| p.parent().map(|p| p.to_path_buf()).unwrap_or(p))
        .unwrap_or_else(|_| current_dir().unwrap())
        .join("static")
});

pub fn service() -> impl HttpServiceFactory {
    Files::new("/", SERVICE_DIRECTORY.clone())
        .index_file("index.html")
        .default_handler(fn_service(|req: ServiceRequest| async {
            if req.match_info().unprocessed().contains('.') {
                return Ok(req.into_response(HttpResponse::NotFound().finish()));
            }
            let (req, _) = req.into_parts();
            let file = NamedFile::open_async(SERVICE_DIRECTORY.join("index.html")).await?;
            let res = file.into_response(&req);
            Ok(ServiceResponse::new(req, res))
        }))
}
