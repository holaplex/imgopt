//use actix_web::http::Error;
//
use actix_web::{
    error, get,
    http::header::{CacheControl, CacheDirective, HeaderMap},
    http::StatusCode,
    middleware, web,
    web::Data,
    App, HttpRequest, HttpResponse, HttpServer,
};
use anyhow::Result;
use awc::{http::header, http::header::CONTENT_TYPE, Client, Connector};
use image::ImageFormat;
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
    skip_list: Option<Vec<String>>,
}

#[derive(Clone)]
struct Object {
    data: Vec<u8>,
    content_type: mime::Mime,
    name: String,
    service: Service,
    response: Option<String>,
    status: Option<StatusCode>,
    headers: Option<HeaderMap>,
}
impl Object {
    fn new(name: String) -> Self {
        Self {
            name,
            data: Vec::new(),
            content_type: "text/plain".parse::<mime::Mime>().unwrap(),
            service: Service::default(),
            response: None,
            status: None,
            headers: None,
        }
    }
    async fn download(
        &mut self,
        client: &Data<awc::Client>,
        cfg: &Data<AppConfig>,
    ) -> Result<&Object, Box<dyn std::error::Error>> {
        //Try download
        let image_path = format!(
            "{}/base/{}/{}",
            cfg.storage_path, self.service.name, self.name
        );
        let url = format!("{}/{}", self.service.endpoint, self.name);
        let start = Instant::now();
        log::info!("Downloading object from: {}", url);
        self.response = if url.is_empty() {
            log::error!("Error! url not provided");
            Some(String::from("URL not provided"))
        } else {
            None
        };
        let mut res = client
            .get(&url)
            .timeout(Duration::from_secs(cfg.req_timeout))
            .send()
            .await?;

        if !res.status().is_success() {
            log::error!(
                "{} did not return expected object: {} -- Response: {:#?}",
                self.service.name,
                self.name,
                res
            );
        }
        self.status = Some(res.status());
        let payload = res
            .body()
            // expected image is larger than default body limit
            .limit(cfg.max_body_size_bytes)
            .await?;
        log::info!(
            "it took {} to download object to memory",
            Elapsed::from(&start)
        );
        //response to bytes
        self.data = payload.as_ref().to_vec();
        //retrieving mime type from headers
        self.headers = Some(res.headers().clone());
        self.content_type = match self.headers.clone().unwrap().get(CONTENT_TYPE) {
            None => {
                log::warn!("The response does not contain a Content-Type header.");
                "application/octet-stream".parse::<mime::Mime>().unwrap()
            }
            Some(x) => x.to_str()?.parse::<mime::Mime>().unwrap(),
        };
        log::info!("got mime from headers: {}", self.content_type);
        //Saving downloaded image in S3
        let start = Instant::now();

        let mut file = File::create(&image_path)?;
        file.write_all(&self.data).unwrap();
        log::info!("it took {} to save image to disk", Elapsed::from(&start));
        //return image as bytes to use from mem
        Ok(self)
    }
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
            skip_list: None,
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
    engine: Option<u32>,
}

async fn get_health_status() -> HttpResponse {
    HttpResponse::Ok().content_type("text/plain").body("200 OK")
}

#[get("/proxy/{service}/{image}")]
async fn forward(
    req: HttpRequest,
    payload: web::Payload,
    client: web::Data<Client>,
    cfg: Data<AppConfig>,
    data: web::Path<(String, String)>,
) -> Result<HttpResponse, Box<dyn std::error::Error>> {
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
    let image = data.1.to_string();
    let url = format!("{}/{}", service.endpoint, image);

    let forwarded_req = client.request_from(url, req.head()).no_decompress();
    //let forwarded_req = match req.head().peer_addr {
    //    Some(addr) => forwarded_req.insert_header(("x-forwarded-for", format!("{}", addr.ip()))),
    //    None => forwarded_req,
    //};

    let res = forwarded_req
        .send_stream(payload)
        .await
        .map_err(error::ErrorInternalServerError)?;

    let mut client_resp = HttpResponse::build(res.status());
    // Remove `Connection` as per
    // https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Connection#Directives
    for (header_name, header_value) in res.headers().iter().filter(|(h, _)| *h != "connection") {
        client_resp.insert_header((header_name.clone(), header_value.clone()));
    }

    Ok(client_resp.streaming(res))
}

#[get("/{service}/{image}")]
async fn fetch_image(
    req: HttpRequest,
    client: Data<Client>,
    cfg: Data<AppConfig>,
    data: web::Path<(String, String)>,
) -> Result<HttpResponse, Box<dyn std::error::Error>> {
    //load config
    let data = data.into_inner();
    let mut obj = Object::new(data.1);
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
    obj.service = service.unwrap().clone();
    //validate scaling param with allow list in config
    let scale: u32 = params.width.unwrap_or(0);
    if let Some(list) = &cfg.allowed_sizes {
        let scale_validation: Vec<_> = list.iter().filter(|&s| s == &scale || scale == 0).collect();
        if scale_validation.get(0).is_none() {
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
    create_dirs(&cfg, &obj, &scale)?;

    let mod_image_path = format!(
        "{}/mod/{}/{}/{}",
        cfg.storage_path, obj.service.name, scale, obj.name
    );
    let force_download = params.force.unwrap_or(false);
    let engine = params.engine.unwrap_or(0);

    //Try opening from mod image path first - if file is not found continue
    //This assumes the stored file is valid.
    //File validation is performed after first download.
    if scale != 0 && !force_download && std::path::Path::new(&mod_image_path).exists() {
        let image_data = utils::read_from_file(&mod_image_path)?;
        obj.content_type = utils::guess_content_type(&mod_image_path)?;
        return Ok(HttpResponse::Ok()
            .insert_header(CacheControl(vec![CacheDirective::MaxAge(31536000u32)]))
            .content_type(obj.content_type)
            .body(image_data));
    };

    let image_path = format!(
        "{}/base/{}/{}",
        cfg.storage_path, obj.service.name, obj.name
    );

    //try to get base image from S3  || download if not found
    let mut obj = if std::path::Path::new(&image_path).exists() && !force_download {
        log::info!("Found object in storage, reading file");
        obj.data = utils::read_from_file(&image_path)?;
        obj.content_type = utils::guess_content_type(&image_path)?;
        obj
    } else {
        obj.download(&client, &cfg).await?.clone()
    };

    //validate response
    if let Some(s) = obj.status {
        if !s.is_success() {
            log::warn!("Bad response when downloading object. Triggering new download");
            obj.download(&client, &cfg).await?;
            //return error 500 if it fails again
            //TODO: Implement proxying to ipfs directly
            if let Some(s) = obj.status {
                if !s.is_success() {
                    log::error!("Error connecting to {}", obj.service.name);
                    return Ok(HttpResponse::InternalServerError().finish());
                }
            }
        }
    };

    // TODO: Try to read(decode as media content) the file based on the content_type.
    // if the file cannot be read successfully trigger an invalidation and
    // download the file again  before serving.

    //convert to png and save as base if mime type = svg
    if obj.content_type == mime::IMAGE_SVG {
        let converted = img::svg_to_png(&obj.data)?;
        obj.data = converted;
        obj.content_type = mime::IMAGE_PNG;
    };

    //send base image if scale is 0
    if scale == 0 {
        return Ok(HttpResponse::Ok()
            .insert_header(CacheControl(vec![CacheDirective::MaxAge(31536000u32)]))
            .content_type(obj.content_type)
            .body(obj.data));
    }

    //Skip processing for images in 'skip_list' array in config file.
    if let Some(list) = &cfg.skip_list {
        let file_validation: Vec<_> = list.iter().filter(|&i| i == &obj.name).collect();
        if file_validation.get(0).is_some() {
            log::info!(
                "Skipping image {}/{} from processing",
                obj.service.name,
                obj.name
            );
            return Ok(HttpResponse::Ok()
                .insert_header(CacheControl(vec![CacheDirective::MaxAge(31536000u32)]))
                .content_type(obj.content_type)
                .body(obj.data.clone()));
        };
    };

    //process the image and return content as bytes
    let data = match obj.content_type.as_ref() {
        "image/jpeg" | "image/jpg" => img::scaledown_static(&obj.data, scale, ImageFormat::Jpeg),
        "image/png" => match engine {
            1 => img::scaledown_static(&obj.data, scale, ImageFormat::Png),
            _ => img::scaledown_png(&obj.data, scale),
        },
        "image/webp" => img::scaledown_static(&obj.data, scale, ImageFormat::WebP),
        "image/gif" => img::scaledown_gif(&image_path, &mod_image_path, scale),
        "image/svg+xml" => {
            obj.content_type = mime::IMAGE_PNG;
            img::scaledown_static(&obj.data, scale, ImageFormat::Png)
        }
        "video/mp4" => {
            obj.content_type = mime::IMAGE_GIF;
            img::mp4_to_gif(&image_path, &mod_image_path, scale)
        }
        "text/html" | "text/plain" => {
            //download probably failed. try again
            log::error!(
                "Object is not a valid image. Re-downloading from service: {}/{}",
                obj.service.name,
                obj.name
            );
            let obj = obj.download(&client, &cfg).await?.clone();
            Ok(obj.data)
        }
        "application/octet-stream" => {
            log::warn!(
                "Got unsupported format: {} - Trying to guess format from base.",
                obj.content_type
            );
            obj.content_type = utils::guess_content_type(&image_path)?;
            Ok(obj.data.clone())
        }
        _ => {
            log::warn!(
                "Got unsupported format: {} - Skipping processing",
                obj.content_type
            );
            Ok(obj.data.clone())
        }
    };

    //if procesing returned Ok, send that as payload.
    //if processing failed, send base image without processing
    let payload = match data {
        Ok(k) => k,
        Err(e) => {
            log::error!(
                "Error reading/decoding file {}/{} | {}",
                obj.service.name,
                obj.name,
                e
            );
            // "error handling" lol
            //attempt to download file again. possibly corrupted file.
            let error = e.to_string();
            if error.contains("buffer") || error.contains("unexpected EOF") {
                log::warn!(
                    "Re-downloading base object: {}/{}",
                    obj.service.name,
                    obj.name
                );
                let obj = obj.download(&client, &cfg).await?.clone();
                obj.data
            } else {
                //probably failed to decode the image, return original.
                obj.data.clone()
            }
        }
    };
    //saving modified image to mod path
    if payload != obj.data {
        //save by width for quick caching
        utils::write_to_file(payload.clone(), &mod_image_path)?;
        //save to latest modified for fixed path
        let latest_mod_path = format!(
            "{}/mod/latest/{}/{}",
            cfg.storage_path, obj.service.name, obj.name
        );
        utils::write_to_file(payload.clone(), &latest_mod_path)?;
    }

    Ok(HttpResponse::Ok()
        .insert_header(CacheControl(vec![CacheDirective::MaxAge(31536000u32)]))
        .content_type(obj.content_type)
        .body(payload))
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
            .service(forward)
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

fn create_dirs(cfg: &AppConfig, obj: &Object, scale: &u32) -> Result<()> {
    fs::create_dir_all(format!("{}/base/{}", cfg.storage_path, obj.service.name))?;
    fs::create_dir_all(format!(
        "{}/mod/latest/{}",
        cfg.storage_path, obj.service.name
    ))?;
    fs::create_dir_all(format!(
        "{}/mod/{}/{}",
        cfg.storage_path, obj.service.name, scale
    ))?;

    Ok(())
}
