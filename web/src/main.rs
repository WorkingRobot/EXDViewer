mod config;
mod crons;
mod data;
mod routes;

use ::config::{Config, Environment, File, FileFormat};
use actix_cors::Cors;
use actix_web::{
    App, HttpServer,
    middleware::{Logger, NormalizePath, TrailingSlash},
    web::Data,
};
use actix_web_prom::PrometheusMetricsBuilder;
use data::GameData;
use prometheus::Registry;
use std::{io, path::PathBuf, sync::Arc};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ServerError {
    #[error("Join error")]
    JoinError(#[from] tokio::task::JoinError),
    #[error("Actix error")]
    ActixError(#[from] io::Error),
    #[error("Dotenvy error")]
    DotenvyError(#[from] dotenvy::Error),
    #[error("Prometheus error")]
    PrometheusError(#[from] prometheus::Error),
}

#[tokio::main]
async fn main() -> Result<(), ServerError> {
    #[cfg(debug_assertions)]
    {
        _ = dotenvy::from_filename(".env");
        _ = dotenvy::from_filename(".secrets.env");
        unsafe { std::env::set_var("RUST_BACKTRACE", "1") };
    }

    let config: config::Config = Config::builder()
        .add_source(File::new("config", FileFormat::Yaml))
        .add_source(Environment::default())
        .build()
        .and_then(Config::try_deserialize)
        .unwrap();

    env_logger::init_from_env(
        env_logger::Env::new()
            .default_filter_or(config.log_filter.clone().unwrap_or("info".to_string())),
    );

    let game_data = Arc::new(GameData::new(
        PathBuf::from(&config.downloader.storage_dir),
        4,
        60,
        50,
        5,
    ));

    let update_game_data_token = crons::create_cron_job(
        crons::UpdateGameData::new(config.downloader.clone())
            .expect("Failed to create UpdateGameData cron job"),
    );

    let prometheus_registry = Registry::new();

    let server_prometheus = PrometheusMetricsBuilder::new("public")
        .registry(prometheus_registry.clone())
        .build()
        .map_err(|e| {
            *e.downcast::<prometheus::Error>()
                .expect("Unknown error from prometheus builder")
        })?;
    let server_config = config.clone();
    let server = HttpServer::new(move || {
        App::new()
            .wrap(Cors::default())
            .wrap(NormalizePath::new(TrailingSlash::Always))
            .wrap(server_prometheus.clone())
            .wrap(
                server_config
                    .log_access_format
                    .as_deref()
                    .map_or_else(Logger::default, Logger::new),
            )
            .app_data(Data::new(server_config.clone()))
            .app_data(Data::new(game_data.clone()))
            .service(routes::api::service())
            .service(routes::assets::service())
    })
    .bind(config.server_addr.clone())?
    .run();

    log::info!("Http server running at http://{}", config.server_addr);

    let private_prometheus = PrometheusMetricsBuilder::new("private")
        .registry(prometheus_registry)
        .endpoint("/metrics")
        .build()
        .map_err(|e| {
            *e.downcast::<prometheus::Error>()
                .expect("Unknown error from prometheus builder")
        })?;
    let prometheus_server = HttpServer::new(move || {
        App::new().wrap(private_prometheus.clone()).wrap(
            config
                .log_access_format
                .as_deref()
                .map_or_else(Logger::default, Logger::new),
        )
    })
    .workers(1)
    .bind(config.metrics_server_addr.clone())?
    .run();

    log::info!(
        "Metrics http server running at http://{}",
        config.metrics_server_addr
    );

    let server_task = tokio::task::spawn(server);
    let prometheus_server_task = tokio::task::spawn(prometheus_server);

    let server_ret = server_task.await;

    update_game_data_token.cancel();
    let prometheus_server_ret = prometheus_server_task.await;

    server_ret??;
    prometheus_server_ret??;

    log::info!("Goodbye!");

    Ok(())
}
