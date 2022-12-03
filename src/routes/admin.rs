use super::ErrorResponse;
use crate::config::AppConfig;
use actix_web::{
    web::{Data, Json},
    HttpResponse,
};
use std::collections::HashMap;

use crate::object::{invalid_value, Object};

use anyhow::{anyhow, Result};
use aws_sdk_cloudfront as cloudfront;
use aws_sdk_cloudfront::model::{invalidation_batch, paths};
use aws_smithy_types::date_time::Format;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use url::Url;

#[derive(Debug, Deserialize)]
pub struct InvalidationReq {
    urls: Vec<String>,
}
#[derive(Debug, Deserialize, Serialize)]
pub struct InvalidationResponse {
    id: String,
    location: String,
    created: String,
    status: InvalidationStatus,
    paths: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
enum InvalidationStatus {
    InProgress,
    Completed,
}
impl Default for InvalidationStatus {
    fn default() -> Self {
        Self::InProgress
    }
}

impl FromStr for InvalidationStatus {
    type Err = ();
    fn from_str(input: &str) -> Result<InvalidationStatus, Self::Err> {
        match input {
            "Completed" => Ok(InvalidationStatus::Completed),
            "InProgress" => Ok(InvalidationStatus::InProgress),
            _ => Err(()),
        }
    }
}
impl ToString for InvalidationStatus {
    fn to_string(&self) -> String {
        match self {
            Self::Completed => "completed".to_string(),
            Self::InProgress => "in progress".to_string(),
        }
    }
}
impl InvalidationResponse {
    fn from_output(o: cloudfront::output::CreateInvalidationOutput) -> Result<Self> {
        Ok(Self {
            location: o.location().map(|l| l.to_string()).unwrap(),
            id: o
                .invalidation
                .as_ref()
                .ok_or(anyhow!("error reading invalidation id"))?
                .id()
                .map(|l| l.to_string())
                .ok_or(anyhow!("error while converting id to string"))?,
            status: InvalidationStatus::from_str(
                o.invalidation
                    .as_ref()
                    .ok_or(anyhow!("error reading invalidation status"))?
                    .status()
                    .ok_or(anyhow!("unable to get invalidation status"))?,
            )
            .expect("Error while converting invalidation status to enum"),
            created: o
                .invalidation
                .as_ref()
                .ok_or(anyhow!("error reading invalidation create time"))?
                .create_time()
                .ok_or(anyhow!("unable to get create time"))?
                .fmt(Format::DateTime)?,
            paths: o
                .invalidation
                .as_ref()
                .ok_or(anyhow!("error reading invalidation paths"))?
                .invalidation_batch()
                .ok_or(anyhow!("unable to get invalidation batch"))?
                .paths()
                .ok_or(anyhow!("unable to get invalidation paths"))?
                .items()
                .ok_or(anyhow!("unable to get invalidation items"))?
                .to_vec(),
        })
    }
}

pub async fn create_invalidation(
    client: Data<awc::Client>,
    cf_client: Data<cloudfront::Client>,
    cfg: Data<AppConfig>,
    data: Option<Json<InvalidationReq>>,
) -> Result<HttpResponse, Box<dyn std::error::Error>> {
    let distribution_id = if let Some(cf) = &cfg.cloudfront {
        &cf.distribution_id
    } else {
        return Ok(HttpResponse::BadRequest().json(ErrorResponse::new(
            400,
            "Distribution ID not found in config. Please add cloudfront.distribution_id = <id> to your config file.",
        )));
    };
    let urls = if let Some(r) = data {
        r.urls.clone()
    } else {
        return Ok(HttpResponse::BadRequest().json(ErrorResponse::new(
            400,
            "Missing urls vec to invalidate. Ex: { urls: [\"https://assets.holaplex.tools/ipfs/<cid>?width=400&path=test.png\"] }",
        )));
    };
    let mut objects: Vec<Object> = Vec::new();
    //Create objects.
    for url in urls.iter() {
        match Url::parse(url) {
            Ok(url) => {
                let pairs: HashMap<_, _> = url.query_pairs().into_owned().collect();
                let scale = pairs
                    .get("width")
                    .unwrap_or(&"0".to_string())
                    .parse::<u32>()?;
                if let Some(q) = pairs.get("url") {
                    let mut obj = Object::from_url(q.to_string());
                    obj.scale(scale).set_paths(&cfg.storage_path);
                    objects.push(obj)
                } else {
                    let mut paths = url.path_segments().unwrap();
                    let got_origin = &paths.next().unwrap_or_default();
                    let origin = match cfg.validate_origin(got_origin) {
                        Some(o) => o,
                        None => return Ok(invalid_value("origin", got_origin.to_string())),
                    };
                    let filename = paths.next().unwrap_or_default();
                    let mut obj = Object::new(filename);
                    obj.origin(&origin).scale(scale);
                    if let Some(path) = &pairs.get("path") {
                        obj.rename(path);
                    };
                    obj.set_paths(&cfg.storage_path);
                    objects.push(obj)
                };
            }
            Err(e) => {
                return Ok(HttpResponse::BadRequest().json(ErrorResponse::new(
                    400,
                    &format!("URL Parse error: {} -- URL: {}", e, url),
                )));
            }
        };
    }

    let mut inv_paths = Vec::new();
    for obj in objects.iter_mut() {
        inv_paths.push(obj.get_cf_path());
        obj.reset_retries(&client, &cfg).await?;
        obj.remove_paths()?;
    }
    let payload = if !inv_paths.is_empty() {
        let paths = paths::Builder::default()
            .set_items(Some(inv_paths.clone()))
            .set_quantity(Some(inv_paths.len().try_into()?))
            .build();
        let batch = invalidation_batch::Builder::default()
            .paths(paths)
            .set_caller_reference(Some(format!("{}", chrono::Utc::now().timestamp())))
            .build();

        let res = cf_client
            .create_invalidation()
            .distribution_id(distribution_id)
            .set_invalidation_batch(Some(batch))
            .send()
            .await?;
        InvalidationResponse::from_output(res)?
    } else {
        return Ok(HttpResponse::BadRequest()
            .json(ErrorResponse::new(400, "No valid paths to invalidate")));
    };
    Ok(HttpResponse::Ok().json(payload))
}
