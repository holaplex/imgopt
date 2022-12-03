use actix_cors::Cors;
use actix_web::{
    guard,
    http::{self, header::HeaderMap, StatusCode},
    middleware, web,
    web::Data,
    App, HttpResponse, HttpServer,
};
use awc::{http::header, http::header::CONTENT_TYPE, Client, Connector};
use aws_config::meta::region::RegionProviderChain;
use aws_sdk_cloudfront as cloudfront;
use config::AppConfig;
use routes::{admin, public};
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
    let config_path = env::var("CONFIG_PATH").unwrap_or(format!("{}/config.toml", path.display()));
    let cfg: AppConfig = confy::load_path(&config_path).unwrap_or_else(|e| {
        println!(
            "
        ==========================
        |[!] ERROR: {e}
        |[.] Loading default config because of above error
        |[.] All fields are required in order to read from config file.
        ==========================
            "
        );
        AppConfig::default()
    });
    let twitter_token = env::var("TWITTER_BEARER_TOKEN").unwrap_or_default();
    let admin_token = env::var("ADMIN_TOKEN").unwrap_or_else(|_| {
        log::warn!("ADMIN_TOKEN env var not set. Using default: admin");
        "admin".to_string()
    });
    let workers = cfg.workers;
    let port = cfg.port;
    env_logger::init_from_env(env_logger::Env::new().default_filter_or(&cfg.log_level));
    log::debug!("The current directory is {}", path.display());
    log::debug!("config loaded: {:#?}", cfg);
    let client_tls_config = Arc::new(config::rustls_config());

    let region_provider = RegionProviderChain::default_provider().or_else("us-east-1");
    let config = aws_config::from_env().region(region_provider).load().await;
    let cf_client = cloudfront::Client::new(&config);

    log::info!("starting HTTP server at http://0.0.0.0:{}", cfg.port);
    HttpServer::new(move || {
        let admin_token = admin_token.clone();
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
                web::get().to(public::get_health_status),
            )
            .wrap(
                Cors::default()
                    .allow_any_origin()
                    .allowed_methods(vec!["GET", "POST"])
                    .allowed_headers(vec![http::header::AUTHORIZATION, http::header::ACCEPT])
                    .allowed_header(http::header::CONTENT_TYPE)
                    .max_age(3600),
            )
            .wrap(middleware::Logger::default())
            .app_data(Data::new(client))
            .app_data(Data::new(twitter_token.clone()))
            .app_data(Data::new(cfg.clone()))
            .app_data(Data::new(cf_client.clone()))
            .service(public::twitter)
            .service(public::get)
            .service(public::fetch_object)
            .service(public::forward)
            .service(
                web::resource("/create_invalidation").route(
                    web::route()
                        .guard(guard::Any(guard::Get()).or(guard::Post()))
                        .guard(guard::fn_guard(move |req| {
                            match req.head().headers.get("authorization") {
                                Some(value) => value == admin_token.clone().as_str(),
                                None => false,
                            }
                        }))
                        .to(admin::create_invalidation),
                ),
            )
    })
    .bind(("0.0.0.0", port))?
    .workers(workers)
    .run()
    .await
}
