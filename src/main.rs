use actix_web::{get, middleware, web, web::Data, App, HttpRequest, HttpResponse, HttpServer};
use anyhow::Result;
use awc::{http::header, Client, Connector};
use image::ImageFormat;
use retry::delay::Fixed;
use retry::OperationResult;
use rustls::{ClientConfig, OwnedTrustAnchor, RootCertStore};
use serde_derive::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::fs::File;
use std::io::prelude::*;
use std::time::Duration;
use std::{sync::Arc, time::Instant};
use utils::Elapsed;
mod img;
mod utils;

#[derive(Debug, Serialize, Deserialize, Clone)]
struct AppConfig {
    port: u16,
    workers: usize,
    log_level: String,
    req_timeout: u64,
    max_body_size_bytes: usize,
    user_agent: String,
    health_endpoint: String,
    storage_path: String,
    allowed_sizes: Option<Vec<u32>>,
    services: Vec<Service>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct Service {
    name: String,
    endpoint: String,
}
impl Default for Service {
    fn default() -> Self {
        Self {
            name: String::from("ipfs"),
            endpoint: String::from("https://ipfs.io/ipfs"),
        }
    }
}
impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            port: 3030,
            workers: 8,
            req_timeout: 15,
            max_body_size_bytes: 60000000,
            log_level: String::from("debug"),
            storage_path: String::from("storage"),
            allowed_sizes: None,
            health_endpoint: String::from("/health"),
            user_agent: format!("{}/{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")),
            services: vec![Service::default()],
        }
    }
}

#[derive(Deserialize)]
pub struct Params {
    width: Option<u32>,
    force: Option<bool>,
}

async fn get_health_status() -> HttpResponse {
    HttpResponse::Ok().content_type("text/plain").body("200 OK")
}

#[get("/{service}/{image}")] // <- define path parameters
async fn fetch_image(
    req: HttpRequest,
    client: Data<Client>,
    cfg: Data<AppConfig>,
    data: web::Path<(String, String)>,
) -> Result<HttpResponse, Box<dyn std::error::Error>> {
    //load config
    let data = data.into_inner();
    let image = data.1;
    //get desired scale from parameters
    let params = web::Query::<Params>::from_query(req.query_string())?;

    //validate service with allow list in config
    let svc = data.0.to_string();
    let service: Option<&Service> = cfg.services.iter().find(|s| s.name == svc);
    if service.is_none() {
        log::warn!("Received endpoint is not allowed");
        return Ok(HttpResponse::BadRequest()
            .content_type("text/plain")
            .body("Received endpoint not allowed!"));
    };
    let service = service.unwrap();
    //validate scaling param with allow list in config
    let scale: u32 = params.width.unwrap_or(0);
    if cfg.allowed_sizes.is_some() {
        let scale_validation: Vec<u32> = cfg
            .allowed_sizes
            .clone()
            .unwrap()
            .into_iter()
            .filter(|s| s == &scale || scale == 0)
            .collect();
        if scale_validation.is_empty() {
            log::warn!(
                "Received parameter not allowed. Got request to scale to {}",
                scale
            );
            return Ok(HttpResponse::BadRequest()
                .content_type("text/plain")
                .body("Scaling value not allowed!"));
        };
    };
    //Creating required directories
    fs::create_dir_all(format!("{}/base/{}", cfg.storage_path, service.name))?;
    fs::create_dir_all(format!("{}/mod/latest/{}", cfg.storage_path, service.name))?;
    fs::create_dir_all(format!(
        "{}/mod/{}/{}",
        cfg.storage_path, service.name, scale
    ))?;

    let uri = format!("{}/{}", service.endpoint, image);

    let mod_image_path = format!(
        "{}/mod/{}/{}/{}",
        cfg.storage_path, service.name, scale, image
    );

    if scale != 0 {
        //Try opening from mod image path first - if file is not found continue
        if std::path::Path::new(&mod_image_path).exists() {
            let image_data = utils::read_from_file(&mod_image_path)?;
            let content_type = utils::guess_content_type(&image_data)?;
            return Ok(HttpResponse::Ok()
                .content_type(content_type)
                .body(image_data));
        };
    }

    //try to get base image from S3  || download if not found
    let image_path = format!("{}/base/{}/{}", cfg.storage_path, service.name, image);
    let image_data: Vec<u8> = if std::path::Path::new(&image_path).exists() {
        log::info!("Found image in storage, reading file");
        utils::read_from_file(&image_path)?
    } else {
        //Try download
        let start = Instant::now();
        log::info!("Trying to download image from: {}", uri);
        if uri.is_empty() {
            log::error!("Error! image not provided");
            return Ok(HttpResponse::InternalServerError().finish());
        }

        let mut res = client
            .get(&uri)
            .timeout(Duration::from_secs(cfg.req_timeout))
            .send()
            .await?;

        if !res.status().is_success() {
            log::error!(
                "{} did not return expected image: {} -- Response: {:#?}",
                service.name,
                image,
                res
            );
            return Ok(HttpResponse::InternalServerError().finish());
        }

        let payload = res
            .body()
            // expected image is larger than default body limit
            .limit(cfg.max_body_size_bytes)
            .await?;
        log::info!(
            "it took {} to download image to memory",
            Elapsed::from(&start)
        );
        //response to bytes
        let data: Vec<u8> = payload.as_ref().to_vec();
        //Saving downloaded image in S3
        let start = Instant::now();
        let mut file = File::create(&image_path)?;
        file.write_all(&data).unwrap();
        log::info!("it took {} to save image to disk", Elapsed::from(&start));
        //return image as bytes to use from mem
        data
    };

    let mut content_type = utils::guess_content_type(&image_data)?;
    if scale == 0 {
        //send base image if scale is 0
        return Ok(HttpResponse::Ok()
            .content_type(content_type)
            .body(image_data));
    }
    //process the image and return payload
    let payload = match content_type.as_ref() {
        "image/jpeg" => img::scaledown_static(&image_data, scale, ImageFormat::Jpeg)?,
        "image/png" => img::scaledown_static(&image_data, scale, ImageFormat::Png)?,
        "image/webp" => img::scaledown_static(&image_data, scale, ImageFormat::WebP)?,
        "image/gif" => img::scaledown_gif(&image_path, &mod_image_path, scale)?,
        "video/mp4" => {
            content_type = mime::IMAGE_GIF;
            img::mp4_to_gif(&image_path, &mod_image_path, scale)?
        }
        _ => {
            log::warn!(
                "Got unsupported format: {} - Skipping processing",
                content_type
            );
            image_data.clone()
        }
    };

    //saving modified image to mod path
    if payload != image_data {
        //save by width for quick caching
        utils::write_to_file(payload.clone(), &mod_image_path)?;
        //save to latest modified for fixed path
        let latest_mod_path = format!("{}/mod/latest/{}/{}", cfg.storage_path, service.name, image);
        utils::write_to_file(payload.clone(), &latest_mod_path)?;
    }
    Ok(HttpResponse::Ok().content_type(content_type).body(payload))
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    //Reading config and initial setup
    let path = env::current_dir()?;
    let config_path = env::var("CONFIG_PATH").unwrap_or(format!("{}/config.toml", path.display()));
    let cfg: AppConfig = confy::load_path(&config_path).unwrap_or_else(|e| {
        println!("==========================");
        println!("ERROR || {}", e);
        println!("Loading default config because of above error");
        println!("All fields are required in order to read from config file.");
        println!("==========================");
        AppConfig::default()
    });

    let workers = cfg.workers;
    let port = cfg.port;

    env_logger::init_from_env(env_logger::Env::new().default_filter_or(&cfg.log_level));
    log::debug!("The current directory is {}", path.display());
    log::debug!("config loaded: {:#?}", cfg);
    let client_tls_config = Arc::new(rustls_config());

    log::info!("starting HTTP server at http://0.0.0.0:{}", cfg.port);

    HttpServer::new(move || {
        // create client _inside_ `HttpServer::new` closure to have one per worker thread
        let client = Client::builder()
            // Adding a User-Agent header to make requests
            .add_default_header((header::USER_AGENT, cfg.user_agent.clone()))
            // a "connector" wraps the stream into an encrypted connection
            .connector(
                Connector::new()
                    .timeout(Duration::from_secs(cfg.req_timeout))
                    .rustls(Arc::clone(&client_tls_config)),
            )
            .finish();
        App::new()
            .route(&cfg.health_endpoint, web::get().to(get_health_status))
            .wrap(middleware::Logger::default())
            .app_data(Data::new(client))
            .app_data(Data::new(cfg.clone()))
            .service(fetch_image)
    })
    .bind(("0.0.0.0", port))?
    .workers(workers)
    .run()
    .await
}

/// Create simple rustls client config from root certificates.
fn rustls_config() -> ClientConfig {
    let mut root_store = RootCertStore::empty();
    root_store.add_server_trust_anchors(webpki_roots::TLS_SERVER_ROOTS.0.iter().map(|ta| {
        OwnedTrustAnchor::from_subject_spki_name_constraints(
            ta.subject,
            ta.spki,
            ta.name_constraints,
        )
    }));

    rustls::ClientConfig::builder()
        .with_safe_defaults()
        .with_root_certificates(root_store)
        .with_no_client_auth()
}
