use std::{fmt::Display, str::FromStr};

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
use serde::{Deserialize, Serialize};
use xiv_core::file::{slug::Slug, version::GameVersion};

use crate::{data::RepositoryInfo, queue::MessageQueue};

pub fn service() -> impl HttpServiceFactory {
    web::scope("/api")
        .service(get_repositories)
        .service(get_versions)
        .service(get_versions_slug)
        .service(get_exists_slug)
        .service(get_file_slug)
        .service(get_file)
        .wrap(
            ErrorHandlers::new()
                .default_handler_client(|r| log_error(true, r))
                .default_handler_server(|r| log_error(false, r)),
        )
}

#[derive(Debug, Clone, Serialize)]
struct RepositoriesInfo {
    repositories: Vec<RepositoryInfo>,
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
            .parse()
            .map_err(|_| serde::de::Error::custom("invalid game version"))
    }
}

impl FromStr for QueryGameVersion {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        if s.eq_ignore_ascii_case("latest") {
            Ok(Self::Latest)
        } else {
            Ok(Self::Specific(GameVersion::new(s)?))
        }
    }
}

impl Display for QueryGameVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QueryGameVersion::Latest => write!(f, "latest"),
            QueryGameVersion::Specific(version) => write!(f, "{version}"),
        }
    }
}

async fn serve_file(
    data: &MessageQueue,
    slug: Option<Slug>,
    version: QueryGameVersion,
    path: String,
) -> Result<HttpResponse> {
    // Handle empty path case
    if path.is_empty() {
        return Err(ErrorBadRequest("File path cannot be empty"));
    }

    let resolved_ver = match &version {
        QueryGameVersion::Latest => None,
        QueryGameVersion::Specific(version) => Some(version.clone()),
    };
    let file_name = path.split_at(path.rfind('/').unwrap_or(0) + 1).1;

    let mut directives = vec![CacheDirective::Public];
    if version != QueryGameVersion::Latest {
        directives.push(CacheDirective::Immutable);
        directives.push(CacheDirective::MaxAge(60 * 60 * 24 * 365));
    } else {
        directives.push(CacheDirective::MaxAge(60 * 60 * 24));
    }

    let data = data.get_file(slug, resolved_ver, path.clone()).await;
    match data {
        Ok(data) => Ok(HttpResponse::Ok()
            .insert_header(ContentDisposition::attachment(file_name))
            .insert_header(CacheControl(directives))
            .body(data.as_ref().clone())),
        Err(err) if matches!(err, ironworks::Error::NotFound(_)) => Err(ErrorBadRequest(err)),
        Err(err) => Err(ErrorInternalServerError(err)),
    }
}

#[get("/{version}/{path:.*}/")]
async fn get_file(
    data: web::Data<MessageQueue>,
    path_info: web::Path<(QueryGameVersion, String)>,
) -> Result<HttpResponse> {
    let (version, path) = path_info.into_inner();
    serve_file(&data, None, version, path).await
}

#[get("/{slug:[0-9a-fA-F]{8}}/{version}/{path:.*}/")]
async fn get_file_slug(
    data: web::Data<MessageQueue>,
    path_info: web::Path<(Slug, QueryGameVersion, String)>,
) -> Result<HttpResponse> {
    let (slug, version, path) = path_info.into_inner();
    serve_file(&data, Some(slug), version, path).await
}

#[derive(Debug, Deserialize)]
struct ExistsQuery {
    /// Comma-separated list of file paths
    files: String,
}

#[derive(Debug, Serialize)]
struct ExistsResponse {
    exists: Vec<bool>,
}

async fn serve_exists(
    data: &MessageQueue,
    slug: Option<Slug>,
    version: QueryGameVersion,
    files_param: &str,
) -> Result<HttpResponse> {
    let files: Vec<String> = files_param
        .split(',')
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .collect();
    if files.is_empty() {
        return Err(ErrorBadRequest("No files specified"));
    }

    let resolved_ver = match &version {
        QueryGameVersion::Latest => None,
        QueryGameVersion::Specific(version) => Some(version.clone()),
    };

    let mut directives = vec![CacheDirective::Public];
    if version != QueryGameVersion::Latest {
        directives.push(CacheDirective::Immutable);
        directives.push(CacheDirective::MaxAge(60 * 60 * 24 * 365));
    } else {
        directives.push(CacheDirective::MaxAge(60 * 60 * 24));
    }

    match data.exists(slug, resolved_ver, files).await {
        Ok(exists) => Ok(HttpResponse::Ok()
            .insert_header(CacheControl(directives))
            .json(ExistsResponse { exists })),
        Err(err) if matches!(err, ironworks::Error::NotFound(_)) => Err(ErrorBadRequest(err)),
        Err(err) => Err(ErrorInternalServerError(err)),
    }
}

#[get("/{slug:[0-9a-fA-F]{8}}/{version}/exists/")]
async fn get_exists_slug(
    data: web::Data<MessageQueue>,
    path_info: web::Path<(Slug, QueryGameVersion)>,
    query: web::Query<ExistsQuery>,
) -> Result<HttpResponse> {
    let (slug, version) = path_info.into_inner();
    serve_exists(&data, Some(slug), version, &query.files).await
}

async fn serve_versions(data: &MessageQueue, slug: Option<Slug>) -> Result<HttpResponse> {
    Ok(HttpResponse::Ok().json(
        data.versions(slug)
            .await
            .ok_or(ErrorBadRequest("No version info available"))?,
    ))
}

#[get("/versions/")]
async fn get_versions(data: web::Data<MessageQueue>) -> Result<HttpResponse> {
    serve_versions(&data, None).await
}

#[get("/{slug:[0-9a-fA-F]{8}}/versions/")]
async fn get_versions_slug(
    data: web::Data<MessageQueue>,
    path_info: web::Path<Slug>,
) -> Result<HttpResponse> {
    serve_versions(&data, Some(path_info.into_inner())).await
}

#[get("/repositories/")]
async fn get_repositories(data: web::Data<MessageQueue>) -> Result<HttpResponse> {
    let repositories = data
        .repositories()
        .await
        .map_err(ErrorInternalServerError)?;
    Ok(HttpResponse::Ok().json(RepositoriesInfo { repositories }))
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
