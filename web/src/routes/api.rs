use std::sync::Arc;

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
use serde::Deserialize;

use crate::data::{GameData, GameVersion};

pub fn service() -> impl HttpServiceFactory {
    web::scope("/api").service(get_file).wrap(
        ErrorHandlers::new()
            .default_handler_client(|r| log_error(true, r))
            .default_handler_server(|r| log_error(false, r)),
    )
}

#[derive(Debug, Deserialize)]
struct FileQuery {
    pub path: String,
    #[serde(default)]
    pub version: GameVersion,
}

#[get("/")]
async fn get_file(
    data: web::Data<Arc<GameData>>,
    query: web::Query<FileQuery>,
) -> Result<HttpResponse> {
    let query = query.into_inner();
    let data = data.get(query.version, query.path.clone());
    match data {
        Ok(data) => Ok(HttpResponse::Ok()
            .insert_header(ContentDisposition::attachment(
                query.path.split_at(query.path.rfind('/').unwrap_or(0)).1,
            ))
            .body(data.as_ref().clone())),
        Err(err) if matches!(err, ironworks::Error::NotFound(_)) => Err(ErrorBadRequest(err)),
        Err(err) => Err(ErrorInternalServerError(err)),
    }
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
