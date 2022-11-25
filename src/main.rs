use actix_cors::Cors;
use actix_web::{
    http::{self, header::HeaderMap, StatusCode},
    middleware, web,
    web::Data,
    App, HttpResponse, HttpServer,
};
use awc::{http::header, http::header::CONTENT_TYPE, Client, Connector};
use config::AppConfig;
use std::{
    env,
    sync::Arc,
    time::{Duration, Instant},
};
mod config;
mod img;
mod object;
mod routes;
mod tw;
mod utils;

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let path = env::current_dir()?;
    let twitter_token = env::var("TWITTER_BEARER_TOKEN").unwrap_or_default();
    let config_path = env::var("CONFIG_PATH").unwrap_or(format!("{}/config.toml", path.display()));
    let cfg: AppConfig = confy::load_path(&config_path).unwrap_or_else(|e| {
        println!(
            "
        ==========================
        |[!] ERROR: {e}
        |[~] Loading default config because of above error
        |[~] All fields are required in order to read from config file.
        ==========================
             "
        );
        AppConfig::default()
    });

    let workers = cfg.workers;
    let port = cfg.port;
    env_logger::init_from_env(env_logger::Env::new().default_filter_or(&cfg.log_level));
    log::debug!("The current directory is {}", path.display());
    log::debug!("config loaded: {:#?}", cfg);
    let client_tls_config = Arc::new(config::rustls_config());

    log::info!("starting HTTP server at http://0.0.0.0:{}", cfg.port);

    HttpServer::new(move || {
        let client = Client::builder()
            .add_default_header((header::USER_AGENT, cfg.user_agent.clone()))
            .connector(
                Connector::new()
                    .timeout(Duration::from_secs(cfg.req_timeout))
                    .rustls(Arc::clone(&client_tls_config)),
            )
            .finish();
        App::new()
            .route(
                &cfg.health_endpoint,
                web::get().to(routes::get_health_status),
            )
            .wrap(
                Cors::default()
                    .allow_any_origin()
                    .allowed_methods(vec!["GET"])
                    .allowed_headers(vec![http::header::AUTHORIZATION, http::header::ACCEPT])
                    .allowed_header(http::header::CONTENT_TYPE)
                    .max_age(3600),
            )
            .wrap(middleware::Logger::default())
            .app_data(Data::new(client))
            .app_data(Data::new(twitter_token.clone()))
            .app_data(Data::new(cfg.clone()))
            .service(routes::twitter)
            .service(routes::get)
            .service(routes::fetch_object)
            .service(routes::forward)
    })
    .bind(("0.0.0.0", port))?
    .workers(workers)
    .run()
    .await
}
