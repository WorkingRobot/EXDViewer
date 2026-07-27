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

const CLIENT_ROUTES: &[&str] = &["sheet", "assets", "music", "auth"];

fn routed_by_client(path: &str) -> bool {
    let segment = path.trim_start_matches('/').split('/').next();
    segment.is_some_and(|segment| CLIENT_ROUTES.contains(&segment))
}

pub fn service() -> impl HttpServiceFactory {
    Files::new("/", SERVICE_DIRECTORY.clone())
        .index_file("index.html")
        .default_handler(fn_service(|req: ServiceRequest| async {
            let path = req.match_info().unprocessed();
            if path.contains('.') && !routed_by_client(path) {
                return Ok(req.into_response(HttpResponse::NotFound().finish()));
            }
            let (req, _) = req.into_parts();
            let file = NamedFile::open_async(SERVICE_DIRECTORY.join("index.html")).await?;
            let res = file.into_response(&req);
            Ok(ServiceResponse::new(req, res))
        }))
}

#[cfg(test)]
mod tests {
    use super::routed_by_client;

    #[test]
    fn client_deep_links_are_pages_not_missing_files() {
        for path in [
            "assets/exd/root.exl",
            "/assets/music/ffxiv/BGM_Null.scd",
            "sheet/Item.foo",
            "music/1",
            "auth/github/callback",
        ] {
            assert!(routed_by_client(path), "{path}");
        }
    }

    #[test]
    fn everything_else_still_reports_a_missing_file() {
        for path in ["nope.js", "/viewer_bg.wasm", "", "assetsfoo/x.js"] {
            assert!(!routed_by_client(path), "{path}");
        }
    }
}
