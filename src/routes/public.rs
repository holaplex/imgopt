use super::ErrorResponse;
use crate::config::AppConfig;
use crate::object::{invalid_value, Object};
use crate::tw::TwitterProfile;
use actix_web::{
    error, get,
    http::header::{CacheControl, CacheDirective},
    web::{self, Data},
    HttpRequest, HttpResponse,
};
use awc::Client;
use serde::Deserialize;
use std::{str, time::Duration};
use url::Url;

#[derive(Debug, Deserialize)]
pub struct Params {
    width: Option<u32>,
    force: Option<bool>,
    engine: Option<u32>,
    path: Option<String>,
    url: Option<String>,
}

pub async fn get_health_status() -> HttpResponse {
    HttpResponse::Ok().content_type("text/plain").body("200 OK")
}

#[get("/twitter/{handle}")]
pub async fn twitter(
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
        return Ok(HttpResponse::BadRequest().json(ErrorResponse::new(400, msg)));
    };
    let mut res = client
        .post("https://api.twitter.com/1.1/users/lookup.json")
        .append_header(("Accept", "application/json"))
        .bearer_auth(&auth_token)
        .send_form(&[("screen_name", &handle)])
        .await?
        .json::<Vec<TwitterProfile>>()
        .await?;

    let mut profile = if let Some(e) = &res[0].errors {
        return Ok(HttpResponse::BadRequest().json(&e[0]));
    } else {
        &mut res[0]
    };

    profile.avatar_highres = Some(
        profile
            .avatar_lowres
            .clone()
            .unwrap()
            .replace("_normal", ""),
    );

    Ok(HttpResponse::Ok()
        .insert_header(CacheControl(vec![CacheDirective::MaxAge(
            cfg.twitter.clone().unwrap_or_default().cache.max_age,
        )]))
        .json(profile))
}

#[get("/proxy/{origin}/{filename}")]
pub async fn forward(
    payload: web::Payload,
    client: web::Data<Client>,
    cfg: Data<AppConfig>,
    data: web::Path<(String, String)>,
) -> Result<HttpResponse, Box<dyn std::error::Error>> {
    //validate origin with allow list in config
    let (origin, filename) = data.into_inner();
    let origin = match cfg.validate_origin(&origin) {
        Some(o) => o,
        None => return Ok(invalid_value("origin", origin)),
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
pub async fn get(
    req: HttpRequest,
    client: Data<Client>,
    cfg: Data<AppConfig>,
) -> Result<HttpResponse, Box<dyn std::error::Error>> {
    let params = web::Query::<Params>::from_query(req.query_string())?;
    if !cfg.allow_any_origin {
        return Ok(HttpResponse::BadRequest().json(ErrorResponse::new(
            400,
            "endpoint disabled. Add allow_any_origin=true to your config.toml to enable",
        )));
    }
    let url = if let Some(u) = &params.url {
        let u = match Url::parse(u) {
            Ok(u) => u,
            Err(e) => {
                return Ok(HttpResponse::BadRequest().json(ErrorResponse::new(
                    400,
                    &format!("Unable to parse url: {u} - Error: {e}"),
                )))
            }
        };
        u
    } else {
        return Ok(HttpResponse::BadRequest().json(ErrorResponse::new(
            400,
            "Please provide an url using the '?url=' query parameter",
        )));
    };

    if let Some(u) = cfg.validate_url(url.to_string()) {
        return Ok(HttpResponse::BadRequest().json(ErrorResponse::new(
            400,
            &format!("url {u} found in deny list. skipping"),
        )));
    };

    let scale = match cfg.validate_scale(params.width) {
        Some(s) => s,
        None => return Ok(invalid_value("width", params.width.unwrap().to_string())),
    };

    let mut obj = Object::from_url(url.to_string());
    obj.scale(scale);
    obj.set_paths(&cfg.storage_path)
        .try_open()?
        .create_dir(&cfg.storage_path)?;

    if params.force.unwrap_or(false) || url.query_pairs().count() != 0 || obj.data.is_empty() {
        obj.get_retries(&client, &cfg).await?;
        if obj.should_retry(cfg.max_retries) {
            obj.download(&client, &cfg).await?;
        } else {
            return Ok(obj.skip()?);
        }
    }

    let valid_mod = std::path::Path::new(&obj.paths.modified).exists();

    let (content_type, payload) = if let Some(s) = obj.status {
        match s.is_success() && obj.is_valid() {
            true => {
                if valid_mod || scale == 0 {
                    Ok((obj.content_type.clone(), obj.data.clone()))
                } else {
                    obj.process(params.engine.unwrap_or(0))
                }
            }
            false => {
                obj.remove_paths()?;
                obj.update_retries(&client, &cfg).await?;
                let msg = format!(
                    "Object downloaded from {}/{} is not valid. Trying to proxy to origin",
                    obj.origin.name, obj.name
                );
                return Ok(HttpResponse::InternalServerError().json(ErrorResponse::new(500, &msg)));
            }
        }?
    } else {
        log::warn!("Error connecting to {}", obj.origin.name);
        return Ok(HttpResponse::InternalServerError().finish());
    };

    obj.save(payload.clone())?;

    let res = HttpResponse::Ok()
        .insert_header(CacheControl(vec![CacheDirective::MaxAge(
            obj.origin.cache.max_age,
        )]))
        .content_type(content_type)
        .body(payload);
    Ok(res)
}

#[get("/{origin}/{filename}")]
pub async fn fetch_object(
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
        None => return Ok(invalid_value("origin", origin)),
    };
    //validate scaling param
    let scale = match cfg.validate_scale(params.width) {
        Some(s) => s,
        None => return Ok(invalid_value("width", params.width.unwrap().to_string())),
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
        if obj.should_retry(cfg.max_retries) {
            obj.download(&client, &cfg).await?;
        } else {
            return Ok(obj.skip()?);
        }
    };

    let valid_mod = std::path::Path::new(&obj.paths.modified).exists();
    let (content_type, payload) = if let Some(s) = obj.status {
        match s.is_success() && obj.is_valid() {
            true => {
                if obj.scale == 0 || valid_mod {
                    Ok((obj.content_type.clone(), obj.data.clone()))
                } else {
                    obj.process(params.engine.unwrap_or(0))
                }
            }
            false => {
                obj.remove_paths()?;
                obj.update_retries(&client, &cfg).await?;
                let msg = format!(
                    "Object downloaded from {}/{} is not valid. Trying to proxy to origin",
                    obj.origin.name, obj.name
                );
                return Ok(HttpResponse::InternalServerError().json(ErrorResponse::new(500, &msg)));
            }
        }?
    } else {
        log::warn!("Error connecting to {}", obj.origin.name);
        return Ok(HttpResponse::InternalServerError().finish());
    };

    obj.save(payload.clone())?;

    let res = HttpResponse::Ok()
        .insert_header(CacheControl(vec![CacheDirective::MaxAge(
            obj.origin.cache.max_age,
        )]))
        .content_type(content_type)
        .body(payload);
    Ok(res)
}
