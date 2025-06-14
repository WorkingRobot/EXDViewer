mod config;
mod crons;
mod data;
mod routes;

use ::config::{Config, Environment, File, FileFormat};
use actix_cors::Cors;
use actix_web::{
    App, HttpServer,
    middleware::{Condition, Logger, NormalizePath, TrailingSlash},
    web::Data,
};
use actix_web_helmet::{
    CrossOriginEmbedderPolicy, CrossOriginOpenerPolicy, Helmet, XContentTypeOptions,
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
        .add_source(File::new("config", FileFormat::Yaml).required(false))
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

    log::info!("Binding to {}", config.server_addr);
    let server = HttpServer::new(move || {
        App::new()
            .wrap(
                Helmet::new()
                    // .add(CrossOriginOpenerPolicy::same_origin())
                    // .add(CrossOriginEmbedderPolicy::require_corp())
                    .add(XContentTypeOptions::nosniff()),
            )
            .wrap(
                Cors::default()
                    .allowed_origin("http://localhost:3000")
                    .allowed_origin("http://localhost:8080")
                    .allowed_origin("http://127.0.0.1:3000")
                    .allowed_origin("http://127.0.0.1:8080")
                    .allowed_methods(vec!["GET"])
                    .allowed_headers(vec!["Content-Type"]),
            )
            .wrap(NormalizePath::new(TrailingSlash::Always))
            .wrap(Condition::new(
                server_config.metrics_server_addr.is_some(),
                server_prometheus.clone(),
            ))
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
    let prometheus_server = if let Some(metrics_addr) = &config.metrics_server_addr {
        let ret = HttpServer::new(move || {
            App::new().wrap(private_prometheus.clone()).wrap(
                config
                    .log_access_format
                    .as_deref()
                    .map_or_else(Logger::default, Logger::new),
            )
        })
        .workers(1)
        .bind(metrics_addr)?
        .run();
        log::info!("Metrics http server running at http://{}", metrics_addr);
        Some(ret)
    } else {
        log::info!("Metrics server disabled");
        None
    };

    let server_task = tokio::task::spawn(server);
    let prometheus_server_task = prometheus_server.map(|s| tokio::task::spawn(s));

    let server_ret = server_task.await;

    update_game_data_token.cancel();
    let prometheus_server_ret = match prometheus_server_task {
        Some(task) => task.await,
        None => Ok(Ok(())),
    };

    server_ret??;
    prometheus_server_ret??;

    log::info!("Goodbye!");

    Ok(())
}
