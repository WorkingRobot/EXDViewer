use std::{
    collections::HashMap,
    io::Write,
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, Instant},
};

use actix_web::post;
use actix_web::{
    HttpRequest, HttpResponse, Result,
    body::{EitherBody, MessageBody},
    dev::{HttpServiceFactory, ServiceResponse},
    error::{ErrorBadRequest, ErrorInternalServerError, ErrorNotFound},
    get,
    http::header::{self, ContentDisposition},
    middleware::{ErrorHandlerResponse, ErrorHandlers},
    web::{self, Bytes},
};
use actix_web_lab::header::{CacheControl, CacheDirective};
use flate2::{Compression, write::GzEncoder};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use xiv_core::file::{slug::Slug, version::GameVersion};

use crate::{
    config::Config,
    data::{Region, RepositoryInfo, Target},
    queue::MessageQueue,
};

pub fn service() -> impl HttpServiceFactory {
    web::scope("/api")
        // Literal-prefixed routes first. They cannot collide with the region routes by segment
        // count, but keeping the rule "literals before variables" makes that obvious.
        .service(get_github_oauth_config)
        .service(post_github_oauth_token)
        .service(get_repositories)
        .service(get_regions)
        .service(get_global_paths)
        .service(get_songs)
        .service(get_versions_repo)
        .service(get_latest_repo)
        .service(get_file_repo)
        .service(get_hash_repo)
        .service(get_paths_repo)
        .service(get_exists_repo)
        .service(get_versions_region)
        .service(get_latest_region)
        .service(get_file_region)
        .service(get_hash_region)
        .service(get_paths_region)
        .service(get_exists_region)
        .wrap(
            ErrorHandlers::new()
                .default_handler_client(|r| log_error(true, r))
                .default_handler_server(|r| log_error(false, r)),
        )
}

#[derive(Debug, Clone, Serialize)]
struct RegionsInfo {
    regions: Vec<Region>,
}

#[derive(Debug, Serialize)]
struct RepositoriesInfo {
    repositories: Vec<RepositoryInfo>,
}

/// Every content response is for a pinned version — `latest` is a redirect, never a resource — so
/// there is one cache policy rather than a branch per handler.
fn pinned() -> Vec<CacheDirective> {
    vec![
        CacheDirective::Public,
        CacheDirective::Immutable,
        CacheDirective::MaxAge(60 * 60 * 24 * 365),
    ]
}

/// Reject a target that carries no sqpack data. Boot ships the launcher, so answering with an
/// empty result would be a confident lie.
async fn require_sqpack(data: &MessageQueue, target: Target) -> Result<()> {
    if data.has_sqpack(target).await {
        Ok(())
    } else {
        Err(ErrorNotFound(format!(
            "{target} has no sqpack data; only /versions/ is available for it"
        )))
    }
}

/// Reject a version no repository behind the target ever published, so a typo fails loudly rather
/// than silently backfilling to something older.
async fn check_version(data: &MessageQueue, target: Target, version: &GameVersion) -> Result<()> {
    if data.version_valid(target, version).await {
        Ok(())
    } else {
        Err(ErrorNotFound(format!("{target} has no version {version}")))
    }
}

/// 307 to the resolved version, so `latest` stays usable by hand without ever being a cacheable
/// resource itself. Temporary, because the target legitimately moves.
async fn redirect_latest(
    data: &MessageQueue,
    request: &HttpRequest,
    target: Target,
    prefix: &str,
    rest: &str,
) -> Result<HttpResponse> {
    let latest = data
        .versions_for(target)
        .await
        .ok_or_else(|| ErrorBadRequest("No version info available"))?
        .latest;
    // `rest` is captured without its trailing slash; every route it can land on has one.
    let query = request.query_string();
    let separator = if query.is_empty() { "" } else { "?" };
    let location = format!("/api/{prefix}/{latest}/{rest}/{separator}{query}");
    Ok(HttpResponse::TemporaryRedirect()
        .insert_header((actix_web::http::header::LOCATION, location))
        .insert_header(CacheControl(vec![
            CacheDirective::Public,
            CacheDirective::MaxAge(60),
        ]))
        .finish())
}

async fn serve_file(
    data: &MessageQueue,
    target: Target,
    version: GameVersion,
    path: String,
) -> Result<HttpResponse> {
    if path.is_empty() {
        return Err(ErrorBadRequest("File path cannot be empty"));
    }
    require_sqpack(data, target).await?;
    check_version(data, target, &version).await?;

    let file_name = path.split_at(path.rfind('/').unwrap_or(0) + 1).1;
    let directives = pinned();

    let data = data.get_file(target, Some(version), path.clone()).await;
    match data {
        Ok(data) => Ok(HttpResponse::Ok()
            .insert_header(ContentDisposition::attachment(file_name))
            .insert_header(CacheControl(directives))
            .body(data.as_ref().clone())),
        Err(err) if matches!(err, ironworks::Error::NotFound(_)) => Err(ErrorBadRequest(err)),
        Err(err) => Err(ErrorInternalServerError(err)),
    }
}

#[get("/paths/")]
async fn get_global_paths(
    data: web::Data<MessageQueue>,
    request: HttpRequest,
) -> Result<HttpResponse> {
    let frame = data
        .get_global_paths()
        .await
        .map_err(ErrorInternalServerError)?;
    serve_frame(&request, frame, 60 * 60)
}

async fn serve_presence(
    data: &MessageQueue,
    request: &HttpRequest,
    target: Target,
    version: GameVersion,
) -> Result<HttpResponse> {
    require_sqpack(data, target).await?;
    check_version(data, target, &version).await?;
    let frame = data
        .get_presence(target, Some(version))
        .await
        .map_err(ErrorInternalServerError)?;
    serve_frame(request, frame, 60 * 60 * 24 * 365)
}

fn serve_frame(request: &HttpRequest, frame: Bytes, max_age: u32) -> Result<HttpResponse> {
    let mut directives = vec![CacheDirective::Public, CacheDirective::MaxAge(max_age)];
    if max_age > 60 * 60 {
        directives.insert(1, CacheDirective::Immutable);
    }

    let mut response = HttpResponse::Ok();
    response
        .content_type("application/octet-stream")
        .insert_header(CacheControl(directives))
        .insert_header((header::VARY, "Accept-Encoding"));

    if accepts(request, "zstd") {
        return Ok(response
            .insert_header((header::CONTENT_ENCODING, "zstd"))
            .body(frame));
    }
    let body = pathlist::decompress(&frame).map_err(ErrorInternalServerError)?;
    if accepts(request, "gzip") {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&body).map_err(ErrorInternalServerError)?;
        let gzipped = encoder.finish().map_err(ErrorInternalServerError)?;
        return Ok(response
            .insert_header((header::CONTENT_ENCODING, "gzip"))
            .body(gzipped));
    }
    Ok(response.body(body))
}

fn accepts(request: &HttpRequest, encoding: &str) -> bool {
    request
        .headers()
        .get(header::ACCEPT_ENCODING)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value.split(',').any(|part| {
                let mut fields = part.split(';');
                let name = fields.next().unwrap_or_default().trim();
                name.eq_ignore_ascii_case(encoding) && !fields.any(|f| f.trim() == "q=0")
            })
        })
}

/// Unnamed files have no path, only the hash the index records them under.
async fn serve_hash(
    data: &MessageQueue,
    target: Target,
    version: GameVersion,
    repository: u8,
    category: u8,
    hash: String,
) -> Result<HttpResponse> {
    require_sqpack(data, target).await?;
    check_version(data, target, &version).await?;

    // 16 hex digits is the `.index` form, split into directory and file halves; 8 is the
    // `.index2` whole-path form.
    let hash = match hash.len() {
        16 => u64::from_str_radix(&hash, 16)
            .map(ironworks::sqpack::IndexHash::Split)
            .map_err(|_| ErrorBadRequest("Malformed index hash")),
        8 => u32::from_str_radix(&hash, 16)
            .map(ironworks::sqpack::IndexHash::Whole)
            .map_err(|_| ErrorBadRequest("Malformed index2 hash")),
        _ => Err(ErrorBadRequest(
            "Hash must be 16 hex digits for .index or 8 for .index2",
        )),
    }?;

    match data
        .get_file_by_hash(target, Some(version), repository, category, hash)
        .await
    {
        Ok(data) => Ok(HttpResponse::Ok()
            .insert_header(CacheControl(pinned()))
            .body(data.as_ref().clone())),
        Err(err) if matches!(err, ironworks::Error::NotFound(_)) => Err(ErrorBadRequest(err)),
        Err(err) => Err(ErrorInternalServerError(err)),
    }
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
    target: Target,
    version: GameVersion,
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

    require_sqpack(data, target).await?;
    check_version(data, target, &version).await?;
    let directives = pinned();

    match data.exists(target, Some(version), files).await {
        Ok(exists) => Ok(HttpResponse::Ok()
            .insert_header(CacheControl(directives))
            .json(ExistsResponse { exists })),
        Err(err) if matches!(err, ironworks::Error::NotFound(_)) => Err(ErrorBadRequest(err)),
        Err(err) => Err(ErrorInternalServerError(err)),
    }
}

#[get("/regions/")]
async fn get_regions(data: web::Data<MessageQueue>) -> Result<HttpResponse> {
    let regions = data.regions().await.map_err(ErrorInternalServerError)?;
    Ok(HttpResponse::Ok().json(RegionsInfo { regions }))
}

#[get("/{region}/versions/")]
async fn get_versions_region(
    data: web::Data<MessageQueue>,
    path_info: web::Path<Region>,
) -> Result<HttpResponse> {
    serve_versions(&data, Target::Region(path_info.into_inner())).await
}

#[get("/repo/{slug}/versions/")]
async fn get_versions_repo(
    data: web::Data<MessageQueue>,
    path_info: web::Path<Slug>,
) -> Result<HttpResponse> {
    serve_versions(&data, Target::Repo(path_info.into_inner())).await
}

async fn serve_versions(data: &MessageQueue, target: Target) -> Result<HttpResponse> {
    match data.versions_for(target).await {
        Some(info) => Ok(HttpResponse::Ok().json(info)),
        None => Err(ErrorBadRequest("No version info available")),
    }
}

// Region-keyed routes: the game as a whole. Every sibling under a version is a literal segment,
// so no endpoint can be shadowed by a file path and registration order is not load-bearing.

#[get("/{region}/latest/{rest:.*}/")]
async fn get_latest_region(
    data: web::Data<MessageQueue>,
    request: HttpRequest,
    path_info: web::Path<(Region, String)>,
) -> Result<HttpResponse> {
    let (region, rest) = path_info.into_inner();
    redirect_latest(
        &data,
        &request,
        Target::Region(region),
        &region.to_string(),
        &rest,
    )
    .await
}

#[get("/{region}/{version}/file/{path:.*}/")]
async fn get_file_region(
    data: web::Data<MessageQueue>,
    path_info: web::Path<(Region, GameVersion, String)>,
) -> Result<HttpResponse> {
    let (region, version, path) = path_info.into_inner();
    serve_file(&data, Target::Region(region), version, path).await
}

#[get("/{region}/{version}/hash/{repository}/{category}/{hash}/")]
async fn get_hash_region(
    data: web::Data<MessageQueue>,
    path_info: web::Path<(Region, GameVersion, u8, u8, String)>,
) -> Result<HttpResponse> {
    let (region, version, repository, category, hash) = path_info.into_inner();
    serve_hash(
        &data,
        Target::Region(region),
        version,
        repository,
        category,
        hash,
    )
    .await
}

#[get("/{region}/{version}/paths/")]
async fn get_paths_region(
    data: web::Data<MessageQueue>,
    request: HttpRequest,
    path_info: web::Path<(Region, GameVersion)>,
) -> Result<HttpResponse> {
    let (region, version) = path_info.into_inner();
    serve_presence(&data, &request, Target::Region(region), version).await
}

#[get("/{region}/{version}/exists/")]
async fn get_exists_region(
    data: web::Data<MessageQueue>,
    path_info: web::Path<(Region, GameVersion)>,
    query: web::Query<ExistsQuery>,
) -> Result<HttpResponse> {
    let (region, version) = path_info.into_inner();
    serve_exists(&data, Target::Region(region), version, &query.files).await
}

// Per-repository escape hatch. Structurally distinct from the region routes by segment count, so
// the two can never collide.

#[get("/repo/{slug}/latest/{rest:.*}/")]
async fn get_latest_repo(
    data: web::Data<MessageQueue>,
    request: HttpRequest,
    path_info: web::Path<(Slug, String)>,
) -> Result<HttpResponse> {
    let (slug, rest) = path_info.into_inner();
    redirect_latest(
        &data,
        &request,
        Target::Repo(slug),
        &format!("repo/{slug}"),
        &rest,
    )
    .await
}

#[get("/repo/{slug}/{version}/file/{path:.*}/")]
async fn get_file_repo(
    data: web::Data<MessageQueue>,
    path_info: web::Path<(Slug, GameVersion, String)>,
) -> Result<HttpResponse> {
    let (slug, version, path) = path_info.into_inner();
    serve_file(&data, Target::Repo(slug), version, path).await
}

#[get("/repo/{slug}/{version}/hash/{repository}/{category}/{hash}/")]
async fn get_hash_repo(
    data: web::Data<MessageQueue>,
    path_info: web::Path<(Slug, GameVersion, u8, u8, String)>,
) -> Result<HttpResponse> {
    let (slug, version, repository, category, hash) = path_info.into_inner();
    serve_hash(
        &data,
        Target::Repo(slug),
        version,
        repository,
        category,
        hash,
    )
    .await
}

#[get("/repo/{slug}/{version}/paths/")]
async fn get_paths_repo(
    data: web::Data<MessageQueue>,
    request: HttpRequest,
    path_info: web::Path<(Slug, GameVersion)>,
) -> Result<HttpResponse> {
    let (slug, version) = path_info.into_inner();
    serve_presence(&data, &request, Target::Repo(slug), version).await
}

#[get("/repo/{slug}/{version}/exists/")]
async fn get_exists_repo(
    data: web::Data<MessageQueue>,
    path_info: web::Path<(Slug, GameVersion)>,
    query: web::Query<ExistsQuery>,
) -> Result<HttpResponse> {
    let (slug, version) = path_info.into_inner();
    serve_exists(&data, Target::Repo(slug), version, &query.files).await
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

/// BGM song metadata proxied from OrchestrionPlugin Google Sheet, keyed by BGM row id and language.
const SONGS_SHEET: &str = "https://docs.google.com/spreadsheets/d/1s-xJjxqp6pwS7oewNy1aOQnr3gaJbewvIBbyYchZ6No/gviz/tq?tqx=out:csv&sheet=";
const SONG_SHEETS: [&str; 5] = ["en", "ja", "fr", "de", "zh"];
const SONGS_TTL: Duration = Duration::from_secs(6 * 60 * 60);
type SongsCache = Mutex<HashMap<&'static str, (Instant, Arc<String>)>>;
static SONGS_CACHE: LazyLock<SongsCache> = LazyLock::new(|| Mutex::new(HashMap::new()));

#[get("/songs/{lang}/")]
async fn get_songs(lang: web::Path<String>) -> Result<HttpResponse> {
    let sheet = SONG_SHEETS
        .into_iter()
        .find(|&s| s == lang.as_str())
        .unwrap_or("en");

    let cached = SONGS_CACHE
        .lock()
        .unwrap()
        .get(sheet)
        .filter(|(fetched, _)| fetched.elapsed() < SONGS_TTL)
        .map(|(_, json)| json.clone());

    let json = match cached {
        Some(json) => json,
        None => {
            let json = Arc::new(build_songs(sheet).await.map_err(ErrorInternalServerError)?);
            SONGS_CACHE
                .lock()
                .unwrap()
                .insert(sheet, (Instant::now(), json.clone()));
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

async fn build_songs(sheet: &str) -> anyhow::Result<String> {
    let client = reqwest::Client::new();
    let meta_csv = client
        .get(format!("{SONGS_SHEET}metadata"))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let lang_csv = client
        .get(format!("{SONGS_SHEET}{sheet}"))
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

    // language sheet: id, title, alt title, special mode title, locations, comments
    let mut songs = Map::new();
    for record in csv::Reader::from_reader(lang_csv.as_bytes()).records() {
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
