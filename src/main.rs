use actix_web::{
    error,
    get,
    http::header::{CacheControl, CacheDirective, HeaderMap},
    http::StatusCode,
    middleware,
    web,
    web::Data,
    //web::Form,
    App,
    HttpRequest,
    HttpResponse,
    HttpServer,
};
use awc::{http::header, http::header::CONTENT_TYPE, Client, Connector};
use config::{AppConfig, CacheConfig, Origin};
use object::Object;
use rustls::{ClientConfig, OwnedTrustAnchor, RootCertStore};
use serde_derive::Deserialize;
use serde_json::{json, Value};
use std::env;
use std::str;
use std::time::Duration;
use std::{sync::Arc, time::Instant};
use tw::TwitterProfile;
use url::Url;
mod config;
mod img;
mod object;
mod tw;
mod utils;

#[derive(Debug, Deserialize)]
pub struct Params {
    width: Option<u32>,
    force: Option<bool>,
    engine: Option<u32>,
    path: Option<String>,
    url: Option<String>,
}

async fn get_health_status() -> HttpResponse {
    HttpResponse::Ok().content_type("text/plain").body("200 OK")
}

#[get("/twitter/{handle}")]
async fn twitter(
    client: web::Data<Client>,
    cfg: Data<AppConfig>,
    twitter_token: Data<String>,
    data: web::Path<String>,
) -> Result<HttpResponse, Box<dyn std::error::Error>> {
    let handle = data.to_string();
    let auth_token = if !twitter_token.is_empty() {
        twitter_token.to_string()
    } else {
        let msg = "env var TWITTER_BEARER_TOKEN not found. Twitter endpoint will not work";
        log::warn!("{}", msg);
        let json = json!({
            "status": 400,
            "error": msg

        });
        return Ok(HttpResponse::BadRequest()
            .content_type("application/json")
            .body(serde_json::to_string(&json).unwrap()));
    };

    //Get user data
    let res = client
        .post("https://api.twitter.com/1.1/users/lookup.json")
        .append_header(("Accept", "application/json"))
        .bearer_auth(&auth_token)
        .send_form(&[("screen_name", &handle)])
        .await
        .map_err(error::ErrorInternalServerError)?
        .json::<serde_json::Value>()
        .await?;

    let payload = serde_json::to_string_pretty(&TwitterProfile::build(res))?;

    let cache = if let Some(twitter_cfg) = cfg.twitter.clone() {
        twitter_cfg.cache
    } else {
        CacheConfig::default()
    };
    Ok(HttpResponse::Ok()
        .insert_header(CacheControl(vec![CacheDirective::MaxAge(cache.max_age)]))
        .content_type("application/json")
        .body(payload))
}
#[get("/proxy/{origin}/{filename}")]
async fn forward(
    payload: web::Payload,
    client: web::Data<Client>,
    cfg: Data<AppConfig>,
    data: web::Path<(String, String)>,
) -> Result<HttpResponse, Box<dyn std::error::Error>> {
    //validate origin with allow list in config
    let (origin, filename) = data.into_inner();
    let origin = match cfg.validate_origin(&origin) {
        Some(o) => o,
        None => return Ok(invalid_param("origin", origin)),
    };
    let url = format!("{}/{}", origin.endpoint, filename);
    let res = client
        .get(&url)
        .no_decompress()
        .timeout(Duration::from_secs(30))
        .send_stream(payload)
        .await
        .map_err(error::ErrorInternalServerError)?;

    let mut client_resp = HttpResponse::build(res.status());
    for (header_name, header_value) in res.headers().iter().filter(|(h, _)| *h != "connection") {
        client_resp.insert_header((header_name.clone(), header_value.clone()));
    }
    Ok(client_resp.streaming(res))
}

#[get("/")]
async fn get(
    req: HttpRequest,
    client: Data<Client>,
    cfg: Data<AppConfig>,
) -> Result<HttpResponse, Box<dyn std::error::Error>> {
    let params = web::Query::<Params>::from_query(req.query_string())?;
    let url = if let Some(u) = &params.url {
        let url = match Url::parse(&u) {
            Ok(url) => url,
            Err(e) => {
                let msg = format!(
                    "Unable to parse url: {} | error: {}",
                    params.url.as_ref().unwrap(),
                    e
                );
                let json: Value = json!({
                    "status": 400,
                    "error": msg
                });
                return Ok(HttpResponse::BadRequest()
                    .content_type("application/json")
                    .body(serde_json::to_string(&json).unwrap()));
            }
        };
        url
    } else {
        let json = json!({
            "status": 400,
            "error": "Please provide an url using the '?url=' parameter"
        });
        return Ok(HttpResponse::BadRequest()
            .content_type("application/json")
            .body(serde_json::to_string(&json).unwrap()));
    };

    let scale = match cfg.validate_scale(params.width) {
        Some(s) => s,
        None => return Ok(invalid_param("scale", params.width.unwrap().to_string())),
    };
    let mut segments = url.path_segments().map(|c| c.collect::<Vec<_>>()).unwrap();
    let filename = segments.first().unwrap().to_string();
    let mut obj = Object::new(&filename);

    let origin = Origin {
        name: url.host_str().unwrap().to_string(),
        endpoint: format!("{}://{}", url.scheme(), url.host_str().unwrap().to_string()),
        cache: CacheConfig::default(),
    };
    segments.remove(0);
    obj.origin(&origin).scale(scale);
    obj.rename(&segments.join("/"));
    //Creating required directories
    obj.set_paths(&cfg.storage_path)
        .try_open()?
        .create_dir(&cfg.storage_path)?;

    if params.force.unwrap_or(false) || obj.data.is_empty() {
        obj.get_retries(&client, &cfg).await?;
        if obj.should_retry() {
            obj.download(&client, &cfg).await?;
            obj.update_retries(&client, &cfg).await?;
        } else {
            let json = json!({
                "status": 400,
                "error": format!(
                    "Max retries reached. Skipping"
                )

            });
            log::error!("Max retries for object: {}/{}", obj.origin.endpoint, obj.name);
            return Ok(HttpResponse::BadRequest()
                .content_type("application/json")
                .body(serde_json::to_string(&json)?));
        }
    };
    //validate content
    let valid = !matches!(obj.content_type.as_ref(), "text/plain" | "text/html");

    let (content_type, payload) = if let Some(s) = obj.status {
        match s.is_success() && valid {
            true => match obj.scale {
                0 => Ok((obj.content_type.clone(), obj.data.clone())),
                _ => obj.process(params.engine.unwrap_or(0)),
            },
            false => {
                obj.update_retries(&client, &cfg).await?;
                std::fs::remove_file(&obj.paths.base)?;
                let json = json!({
                    "status": 400,
                    "error": format!(
                        "Object downloaded from {}/{} is not a supported asset",
                        obj.origin.name,
                        obj.name
                    )

                });
                return Ok(HttpResponse::BadRequest()
                    .content_type("application/json")
                    .body(serde_json::to_string(&json)?));
            }
        }?
    } else {
        log::warn!("Error connecting to {}", obj.origin.name);
        return Ok(HttpResponse::InternalServerError().finish());
    };
    //save image to disk if modified
    if payload != obj.data && scale != 0 {
        //save by width for quick caching
        utils::write_to_file(payload.clone(), &obj.paths.modified)?;
    }
    let res = HttpResponse::Ok()
        .insert_header(CacheControl(vec![CacheDirective::MaxAge(
            obj.origin.cache.max_age,
        )]))
        .content_type(content_type)
        .body(payload);
    Ok(res)
}

#[get("/{origin}/{filename}")]
async fn fetch_object(
    req: HttpRequest,
    client: Data<Client>,
    cfg: Data<AppConfig>,
    data: web::Path<(String, String)>,
) -> Result<HttpResponse, Box<dyn std::error::Error>> {
    let (origin, filename) = data.into_inner();
    let params = web::Query::<Params>::from_query(req.query_string())?;

    //Validate origin
    let origin = match cfg.validate_origin(&origin) {
        Some(o) => o,
        None => return Ok(invalid_param("origin", origin)),
    };
    //validate scaling param
    let scale = match cfg.validate_scale(params.width) {
        Some(s) => s,
        None => return Ok(invalid_param("scale", params.width.unwrap().to_string())),
    };
    //init object
    let mut obj = Object::new(&filename);
    obj.origin(&origin).scale(scale);
    if let Some(path) = &params.path {
        obj.rename(path);
    };
    //Creating required directories
    obj.set_paths(&cfg.storage_path)
        .try_open()?
        .create_dir(&cfg.storage_path)?;

    if params.force.unwrap_or(false) || obj.data.is_empty() {
        obj.get_retries(&client, &cfg).await?;
        match obj.should_retry() {
            true => obj.download(&client, &cfg).await?,
            false => {
                let json = json!({
                    "status": 400,
                    "error": format!(
                        "Max retries reached. Skipping"
                    )

                });
            log::error!("Max retries for object: {}/{}", obj.origin.endpoint, obj.name);
            return Ok(HttpResponse::BadRequest()
                .content_type("application/json")
                .body(serde_json::to_string(&json)?));
            }
        };
    }
    //validate content
    let valid = !matches!(obj.content_type.as_ref(), "text/plain" | "text/html");

    let (content_type, payload) = if let Some(s) = obj.status {
        match s.is_success() && valid {
            true => match obj.scale {
                0 => Ok((obj.content_type.clone(), obj.data.clone())),
                _ => obj.process(params.engine.unwrap_or(0)),
            },
            false => {
                //Take note in db, dont retry.

                std::fs::remove_file(&obj.paths.base)?;
                let json = json!({
                    "status": 400,
                    "error": format!(
                        "Object downloaded from {}/{} is not a supported asset",
                        obj.origin.name,
                        obj.name
                    )

                });
                return Ok(HttpResponse::BadRequest()
                    .content_type("application/json")
                    .body(serde_json::to_string(&json)?));
            }
        }?
    } else {
        log::warn!("Error connecting to {}", obj.origin.name);
        return Ok(HttpResponse::InternalServerError().finish());
    };
    //save image to disk if modified
    if payload != obj.data && scale != 0 {
        //save by width for quick caching
        utils::write_to_file(payload.clone(), &obj.paths.modified)?;
    }
    let res = HttpResponse::Ok()
        .insert_header(CacheControl(vec![CacheDirective::MaxAge(
            obj.origin.cache.max_age,
        )]))
        .content_type(content_type)
        .body(payload);
    Ok(res)
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let path = env::current_dir()?;
    let twitter_token = match env::var("TWITTER_BEARER_TOKEN") {
        Ok(val) => val,
        Err(_) => String::new(),
    };
    let config_path = env::var("CONFIG_PATH").unwrap_or(format!("{}/config.toml", path.display()));
    let cfg: AppConfig = confy::load_path(&config_path).unwrap_or_else(|e| {
        log::warn!("==========================");
        log::warn!("ERROR || {}", e);
        log::warn!("Loading default config because of above error");
        log::warn!("All fields are required in order to read from config file.");
        log::warn!("==========================");
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
        let client = Client::builder()
            .add_default_header((header::USER_AGENT, cfg.user_agent.clone()))
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
            .app_data(Data::new(twitter_token.clone()))
            .app_data(Data::new(cfg.clone()))
            .service(twitter)
            .service(get)
            .service(fetch_object)
            .service(forward)
    })
    .bind(("0.0.0.0", port))?
    .workers(workers)
    .run()
    .await
}

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

fn invalid_param(param: &str, value: String) -> HttpResponse {
    let msg = format!("Received {}: {} is not allowed", param, value);
    let json = json!({
        "status": 400,
        "error": msg

    });
    log::warn!("{}", msg);
    HttpResponse::BadRequest()
        .content_type("application/json")
        .body(serde_json::to_string(&json).unwrap())
}
