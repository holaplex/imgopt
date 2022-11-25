use crate::{
    config::{AppConfig, CacheConfig, Origin},
    img,
    routes::ErrorResponse,
    utils::{self, Elapsed},
    CONTENT_TYPE,
    {web::Data, HeaderMap, HttpResponse, StatusCode},
    {Duration, Instant},
};
use actix_web::error as actix_error;
use anyhow::Result;
use derivative::Derivative;
use image::ImageFormat;
use log::{debug, error, info, warn};
use mime::Mime;
use serde::{Deserialize, Serialize};
use std::fs;
use std::fs::File;
use std::io::prelude::*;
use std::str;
use url::Url;

#[derive(Debug, Derivative, Clone)]
#[derivative(Default)]
pub struct Object {
    pub name: String,
    pub url: String,
    pub data: Vec<u8>,
    #[derivative(Default(value = "mime::TEXT_PLAIN"))]
    pub content_type: Mime,
    pub origin: Origin,
    pub scale: u32,
    pub paths: Paths,
    pub retries: u32,
    pub status: Option<StatusCode>,
    pub headers: Option<HeaderMap>,
}

#[derive(Debug, Default, Clone)]
pub struct Paths {
    pub base: String,
    pub modified: String,
}
#[derive(Serialize, Deserialize)]
struct RetryCount {
    url: String,
    retries: u32,
}

impl Object {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            ..Default::default()
        }
    }

    pub fn from_url(url: String) -> Self {
        let mut obj = Object::new(&url);
        obj.url = url.clone();
        obj.origin = Origin {
            name: "misc".to_string(),
            endpoint: url,
            cache: CacheConfig::default(),
        };
        obj.name = obj.get_hash();
        obj
    }

    pub fn create_dir(&self, path: &str) -> Result<()> {
        fs::create_dir_all(format!("{}/base/{}", path, self.origin.name))?;
        if self.scale != 0 {
            fs::create_dir_all(format!("{}/mod/{}/{}", path, self.origin.name, self.scale))?;
        }
        Ok(())
    }

    pub fn try_open(&mut self) -> Result<&Self, Box<dyn std::error::Error>> {
        let valid_base = std::path::Path::new(&self.paths.base).exists();
        let valid_mod = std::path::Path::new(&self.paths.modified).exists();
        self.data = if valid_base && !valid_mod {
            self.content_type = utils::guess_content_type(&self.paths.base)?;
            utils::read_from_file(&self.paths.base)?
        } else if self.scale != 0 && valid_mod {
            self.content_type = utils::guess_content_type(&self.paths.modified)?;
            utils::read_from_file(&self.paths.modified)?
        } else {
            vec![]
        };
        self.status = Some(StatusCode::OK);
        Ok(self)
    }

    pub fn rename(&mut self, path: &str) -> &mut Self {
        self.url = format!("{}/{}/{}", self.origin.endpoint, self.name, path);
        self.name = format!(
            "{}-_-{}",
            self.name,
            path.replace('/', "-_-").replace(' ', "_")
        );
        self
    }

    pub fn set_paths(&mut self, path: &str) -> &mut Self {
        self.paths = Paths {
            modified: if self.scale != 0 {
                format!(
                    "{}/mod/{}/{}/{}",
                    path, self.origin.name, self.scale, self.name
                )
            } else {
                String::new()
            },
            base: format!("{}/base/{}/{}", path, self.origin.name, self.name),
        };
        self
    }

    pub fn origin(&mut self, origin: &Origin) -> &mut Self {
        self.origin = origin.clone();
        self.url = format!("{}/{}/", origin.endpoint, self.name);
        self
    }

    pub fn scale(&mut self, scale: u32) -> &mut Self {
        self.scale = scale;
        self
    }

    pub fn get_hash(&self) -> String {
        sha1_smol::Sha1::from(&self.url.as_bytes())
            .digest()
            .to_string()
    }

    pub fn skip(&self) -> Result<HttpResponse> {
        let msg = format!("Max retries reached for url: {}", self.get_url()?);
        Ok(HttpResponse::BadRequest().json(ErrorResponse::new(400, &msg)))
    }

    pub fn is_valid(&self) -> bool {
        !matches!(self.content_type.as_ref(), "text/plain" | "text/html")
    }

    pub fn save(&self, payload: Vec<u8>) -> Result<()> {
        if payload != self.data && self.scale != 0 {
            utils::write_to_file(payload, &self.paths.modified)?;
        }
        Ok(())
    }

    pub fn should_retry(&self, num: u32) -> bool {
        self.retries < num
    }

    pub fn remove_file(&self) -> Result<HttpResponse> {
        std::fs::remove_file(&self.paths.base)?;
        let msg = format!(
            "Object downloaded from {}/{} is not a supported asset",
            self.origin.name, self.name
        );
        Ok(HttpResponse::BadRequest().json(ErrorResponse::new(400, &msg)))
    }

    pub async fn update_retries(
        &mut self,
        client: &Data<awc::Client>,
        cfg: &Data<AppConfig>,
    ) -> Result<&Self, Box<dyn std::error::Error>> {
        self.retries += 1;
        let hash = self.get_hash();
        let url = Url::parse(&format!("{}/api/{}", cfg.kvstore_uri, hash))?;
        let retries = RetryCount {
            url: self.get_url()?.to_string(),
            retries: self.retries,
        };

        self.retries = client
            .post(url.as_str())
            .append_header(("Accept", "application/json"))
            .timeout(Duration::from_secs(cfg.req_timeout))
            .send_json(&retries)
            .await
            .map_err(actix_error::ErrorInternalServerError)?
            .json::<RetryCount>()
            .await
            .map_err(actix_error::ErrorInternalServerError)?
            .retries;

        Ok(self)
    }

    pub async fn get_retries(
        &mut self,
        client: &Data<awc::Client>,
        cfg: &Data<AppConfig>,
    ) -> Result<&Self, Box<dyn std::error::Error>> {
        let url = Url::parse(&format!("{}/api/{}", cfg.kvstore_uri, self.get_hash()))?;
        let mut res = client
            .get(url.as_str())
            .append_header(("Accept", "application/json"))
            .timeout(Duration::from_secs(cfg.req_timeout))
            .send()
            .await?;
        match res.status() {
            StatusCode::NOT_FOUND => {
                self.update_retries(client, cfg).await?;
            }
            StatusCode::OK => {
                let data = res.json::<RetryCount>().await?;
                self.retries = data.retries;
            }
            StatusCode::INTERNAL_SERVER_ERROR => error!("Error contacting kv store"),
            _ => warn!("Unexpected response from kv store"),
        };
        Ok(self)
    }

    fn get_url(&self) -> Result<Url> {
        Ok(Url::parse(&self.url)?)
    }

    pub async fn download(
        &mut self,
        client: &Data<awc::Client>,
        cfg: &Data<AppConfig>,
    ) -> Result<&Self, Box<dyn std::error::Error>> {
        let url = self.get_url()?;
        let start = Instant::now();
        info!("Downloading from url: {}", url);
        let connector = client
            .get(url.as_str())
            .timeout(Duration::from_secs(cfg.req_timeout))
            .send()
            .await;
        let mut res = match connector {
            Ok(r) => {
                if r.status().is_success() {
                    r
                } else {
                    error!(
                        "Error in response: {} | Origin: {}",
                        r.status(),
                        self.get_url()?,
                    );
                    return Ok(self);
                }
            }
            Err(e) => {
                warn!(
                    "Error while connecting to {} | Error: {}",
                    self.get_url()?,
                    e
                );
                return Ok(self);
            }
        };
        self.status = Some(res.status());
        let payload = res.body().limit(cfg.max_body_size_bytes).await?;
        debug!(
            "it took {} to download object to memory",
            Elapsed::from(&start)
        );
        self.data = payload.as_ref().to_vec();
        self.headers = Some(res.headers().clone());
        self.content_type = match self.headers.clone().unwrap().get(CONTENT_TYPE) {
            None => {
                warn!("The response does not contain a Content-Type header.");
                "application/octet-stream".parse::<mime::Mime>()?
            }
            Some(x) => x.to_str()?.parse::<mime::Mime>()?,
        };
        debug!("mime from headers: {}", self.content_type);
        let start = Instant::now();
        let mut file = File::create(&self.paths.base)?;
        file.write_all(&self.data)?;
        debug!("it took {} to save object to disk", Elapsed::from(&start));
        Ok(self)
    }

    pub fn process(&self, engine: u32) -> Result<(Mime, Vec<u8>)> {
        let scale = self.scale;
        let mut content_type = self.content_type.clone();
        let data = match self.content_type.as_ref() {
            "image/jpeg" | "image/jpg" => img::resize_static(&self.data, scale, ImageFormat::Jpeg),
            "image/png" => match engine {
                1 => img::resize_static(&self.data, scale, ImageFormat::Png),
                _ => img::resize_png(&self.data, scale),
            },
            "image/webp" => img::resize_webp(&self.data, scale, img::is_webp_animated(&self.data)),
            "image/gif" => img::resize_gif(&self.paths.base, &self.paths.modified, scale),
            "image/svg+xml" => {
                content_type = mime::IMAGE_PNG;
                img::resize_static(&img::svg_to_png(&self.data)?, scale, ImageFormat::Png)
            }
            "video/mp4" => {
                content_type = mime::IMAGE_GIF;
                img::mp4_to_gif(&self.paths.base, &self.paths.modified, scale)
            }
            "application/octet-stream" => {
                warn!(
                    "Got unsupported format: {} - Trying to guess format from base.",
                    self.content_type
                );
                content_type =
                    utils::guess_content_type(&self.paths.base).unwrap_or(mime::IMAGE_PNG);
                Ok(self.data.clone())
            }
            "application/json" => Ok(self.data.clone()),
            _ => {
                warn!(
                    "Got unsupported format: {} - Skipping processing",
                    self.content_type
                );
                Ok(self.data.clone())
            }
        };
        Ok((content_type, data?))
    }
}
