use std::{fmt::Display, sync::Arc};

use actix_web::{
    HttpResponse, Result,
    body::{EitherBody, MessageBody},
    dev::{HttpServiceFactory, ServiceResponse},
    error::{ErrorBadRequest, ErrorInternalServerError},
    get,
    http::header::ContentDisposition,
    middleware::{ErrorHandlerResponse, ErrorHandlers},
    web::{self, Bytes},
};
use actix_web_lab::header::{CacheControl, CacheDirective};
use serde::Deserialize;

use crate::data::{GameData, GameVersion};

pub fn service() -> impl HttpServiceFactory {
    web::scope("/api")
        .service(get_file)
        .service(get_versions)
        .wrap(
            ErrorHandlers::new()
                .default_handler_client(|r| log_error(true, r))
                .default_handler_server(|r| log_error(false, r)),
        )
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum QueryGameVersion {
    #[default]
    Latest,
    Specific(GameVersion),
}

impl<'a> Deserialize<'a> for QueryGameVersion {
    fn deserialize<D: serde::Deserializer<'a>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .try_into()
            .map_err(|_| serde::de::Error::custom("invalid game version"))
    }
}

impl TryFrom<String> for QueryGameVersion {
    type Error = ();

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.eq_ignore_ascii_case("latest") {
            Ok(Self::Latest)
        } else {
            Ok(Self::Specific(value.try_into()?))
        }
    }
}

impl Display for QueryGameVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QueryGameVersion::Latest => write!(f, "latest"),
            QueryGameVersion::Specific(version) => write!(f, "{}", version),
        }
    }
}

#[derive(Debug, Deserialize)]
struct FileQuery {
    pub version: QueryGameVersion,
    pub path: String,
}

#[get("/{version}/{path:.+}/")]
async fn get_file(
    data: web::Data<Arc<GameData>>,
    path: web::Path<FileQuery>,
) -> Result<HttpResponse> {
    let FileQuery { version, path } = path.into_inner();

    let resolved_ver = match &version {
        QueryGameVersion::Latest => {
            data.versions()
                .ok_or(ErrorBadRequest("No version info available"))?
                .latest
        }
        QueryGameVersion::Specific(version) => version.clone(),
    };
    let file_name = path.split_at(path.rfind('/').unwrap_or(0) + 1).1;

    let mut directives = vec![CacheDirective::Public];
    if version != QueryGameVersion::Latest {
        directives.push(CacheDirective::Immutable);
        directives.push(CacheDirective::MaxAge(60 * 60 * 24 * 30));
    } else {
        directives.push(CacheDirective::MaxAge(60 * 60 * 24));
    }

    let data = data.get(resolved_ver, path.clone());
    match data {
        Ok(data) => Ok(HttpResponse::Ok()
            .insert_header(ContentDisposition::attachment(file_name))
            .insert_header(CacheControl(directives))
            .body(data.as_ref().clone())),
        Err(err) if matches!(err, ironworks::Error::NotFound(_)) => Err(ErrorBadRequest(err)),
        Err(err) => Err(ErrorInternalServerError(err)),
    }
}

#[get("/versions/")]
async fn get_versions(data: web::Data<Arc<GameData>>) -> Result<HttpResponse> {
    Ok(HttpResponse::Ok().json(
        data.versions()
            .ok_or(ErrorBadRequest("No version info available"))?,
    ))
}

fn log_error<B: MessageBody + 'static>(
    is_client: bool,
    res: ServiceResponse<B>,
) -> actix_web::Result<ErrorHandlerResponse<B>> {
    Ok(ErrorHandlerResponse::Future(Box::pin(log_error2(
        is_client, res,
    ))))
}

async fn log_error2<B: MessageBody + 'static>(
    is_client: bool,
    res: ServiceResponse<B>,
) -> actix_web::Result<ServiceResponse<EitherBody<B>>> {
    let (req, res) = res.into_parts();
    let (res, body) = res.into_parts();

    let body = {
        let data = actix_web::body::to_bytes_limited(body, 1 << 12).await;
        let line = match &data {
            Ok(Ok(data)) => String::from_utf8_lossy(data).into_owned(),
            Ok(Err(_)) => "Error reading body".to_string(),
            Err(_) => "Body too large".to_string(),
        };
        if is_client {
            log::error!("Client Error: {}", line);
        } else {
            log::error!("Server Error: {}", line);
        }

        match data {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(_)) => Bytes::from_static(b"Body conversion failure"),
            Err(_) => Bytes::from_static(b"Body too large"),
        }
    };

    let res = ServiceResponse::new(req, res.map_body(|_head, _body| body))
        .map_into_boxed_body()
        .map_into_right_body();

    Ok(res)
}
