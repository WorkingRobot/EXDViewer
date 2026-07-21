use std::{
    fmt::Display,
    str::FromStr,
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, Instant},
};

use actix_web::post;
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
use serde_json::{Map, Value};
use xiv_core::file::{slug::Slug, version::GameVersion};

use crate::{config::Config, data::RepositoryInfo, queue::MessageQueue};

pub fn service() -> impl HttpServiceFactory {
    web::scope("/api")
        .service(get_github_oauth_config)
        .service(post_github_oauth_token)
        .service(get_repositories)
        .service(get_versions_slug)
        .service(get_exists_slug)
        .service(get_file_slug)
        .service(get_songs)
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
    slug: Slug,
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

#[get("/{slug}/{version}/{path:.*}/")]
async fn get_file_slug(
    data: web::Data<MessageQueue>,
    path_info: web::Path<(Slug, QueryGameVersion, String)>,
) -> Result<HttpResponse> {
    let (slug, version, path) = path_info.into_inner();
    serve_file(&data, slug, version, path).await
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
    slug: Slug,
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

#[get("/{slug}/{version}/exists/")]
async fn get_exists_slug(
    data: web::Data<MessageQueue>,
    path_info: web::Path<(Slug, QueryGameVersion)>,
    query: web::Query<ExistsQuery>,
) -> Result<HttpResponse> {
    let (slug, version) = path_info.into_inner();
    serve_exists(&data, slug, version, &query.files).await
}

async fn serve_versions(data: &MessageQueue, slug: Slug) -> Result<HttpResponse> {
    Ok(HttpResponse::Ok().json(
        data.versions(slug)
            .await
            .ok_or(ErrorBadRequest("No version info available"))?,
    ))
}

#[get("/{slug}/versions/")]
async fn get_versions_slug(
    data: web::Data<MessageQueue>,
    path_info: web::Path<Slug>,
) -> Result<HttpResponse> {
    serve_versions(&data, path_info.into_inner()).await
}

#[get("/repositories/")]
async fn get_repositories(data: web::Data<MessageQueue>) -> Result<HttpResponse> {
    let repositories = data
        .repositories()
        .await
        .map_err(ErrorInternalServerError)?;
    Ok(HttpResponse::Ok().json(RepositoriesInfo { repositories }))
}

#[derive(Debug, Serialize)]
struct GithubOAuthConfig {
    client_id: String,
}

#[get("/github/oauth/config/")]
async fn get_github_oauth_config(config: web::Data<Config>) -> Result<HttpResponse> {
    Ok(HttpResponse::Ok().json(GithubOAuthConfig {
        client_id: config.github_client_id.clone(),
    }))
}

#[derive(Debug, Deserialize)]
struct GithubOAuthRequest {
    code: String,
    code_verifier: Option<String>,
    redirect_uri: Option<String>,
}

#[post("/github/oauth/token/")]
async fn post_github_oauth_token(
    config: web::Data<Config>,
    body: web::Json<GithubOAuthRequest>,
) -> Result<HttpResponse> {
    if config.github_client_id.is_empty() || config.github_client_secret.is_empty() {
        return Err(ErrorInternalServerError("GitHub OAuth is not configured"));
    }

    let mut params = Map::new();
    params.insert(
        "client_id".into(),
        Value::String(config.github_client_id.clone()),
    );
    params.insert(
        "client_secret".into(),
        Value::String(config.github_client_secret.clone()),
    );
    params.insert("code".into(), Value::String(body.code.clone()));
    if let Some(verifier) = &body.code_verifier {
        params.insert("code_verifier".into(), Value::String(verifier.clone()));
    }
    if let Some(redirect_uri) = &body.redirect_uri {
        params.insert("redirect_uri".into(), Value::String(redirect_uri.clone()));
    }

    let response = reqwest::Client::new()
        .post("https://github.com/login/oauth/access_token")
        .header("Accept", "application/json")
        .json(&params)
        .send()
        .await
        .map_err(ErrorInternalServerError)?;

    let value: Value = response.json().await.map_err(ErrorInternalServerError)?;
    Ok(HttpResponse::Ok().json(value))
}

/// BGM song metadata proxied from the OrchestrionPlugin Google Sheet (no CORS headers, so
/// the browser can't fetch it directly), keyed by BGM row id.
const SONGS_SHEET: &str = "https://docs.google.com/spreadsheets/d/1s-xJjxqp6pwS7oewNy1aOQnr3gaJbewvIBbyYchZ6No/gviz/tq?tqx=out:csv&sheet=";
const SONGS_TTL: Duration = Duration::from_secs(6 * 60 * 60);
type SongsCache = Mutex<Option<(Instant, Arc<String>)>>;
static SONGS_CACHE: LazyLock<SongsCache> = LazyLock::new(|| Mutex::new(None));

#[get("/songs/")]
async fn get_songs() -> Result<HttpResponse> {
    let cached = SONGS_CACHE
        .lock()
        .unwrap()
        .as_ref()
        .filter(|(fetched, _)| fetched.elapsed() < SONGS_TTL)
        .map(|(_, json)| json.clone());

    let json = match cached {
        Some(json) => json,
        None => {
            let json = Arc::new(build_songs().await.map_err(ErrorInternalServerError)?);
            *SONGS_CACHE.lock().unwrap() = Some((Instant::now(), json.clone()));
            json
        }
    };

    Ok(HttpResponse::Ok()
        .insert_header(CacheControl(vec![
            CacheDirective::Public,
            CacheDirective::MaxAge(60 * 60 * 6),
        ]))
        .content_type("application/json")
        .body(json.as_ref().clone()))
}

async fn build_songs() -> anyhow::Result<String> {
    let client = reqwest::Client::new();
    let meta_csv = client
        .get(format!("{SONGS_SHEET}metadata"))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let en_csv = client
        .get(format!("{SONGS_SHEET}en"))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;

    // metadata sheet: id, duration (seconds)
    let mut durations = std::collections::HashMap::new();
    for record in csv::Reader::from_reader(meta_csv.as_bytes()).records() {
        let record = record?;
        if let (Some(Ok(id)), Some(Ok(duration))) = (
            record.get(0).map(str::parse::<u32>),
            record.get(1).map(str::parse::<f64>),
        ) {
            durations.insert(id, duration.round() as u64);
        }
    }

    // en sheet: id, title, alt title, special mode title, locations, comments
    let mut songs = Map::new();
    for record in csv::Reader::from_reader(en_csv.as_bytes()).records() {
        let record = record?;
        let Some(Ok(id)) = record.get(0).map(str::parse::<u32>) else {
            continue;
        };
        let title = record.get(1).unwrap_or("").trim();
        if title.is_empty() || title == "None" {
            continue;
        }
        let mut song = Map::new();
        song.insert("t".into(), Value::from(title));
        for (key, column) in [("a", 2), ("s", 3), ("l", 4), ("i", 5)] {
            let value = record.get(column).unwrap_or("").trim();
            if !value.is_empty() {
                song.insert(key.into(), Value::from(value));
            }
        }
        if let Some(&duration) = durations.get(&id).filter(|&&d| d > 0) {
            song.insert("d".into(), Value::from(duration));
        }
        songs.insert(id.to_string(), Value::Object(song));
    }

    Ok(serde_json::to_string(&Value::Object(songs))?)
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
