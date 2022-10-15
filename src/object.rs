use crate::{
    config::{AppConfig, Origin},
    img, json,
    utils::{self, Elapsed},
    Url, CONTENT_TYPE,
    {web::Data, HeaderMap, HttpResponse, StatusCode},
    {Duration, Instant},
};
use anyhow::Result;
use image::ImageFormat;
use mime::Mime;
use std::fs;
use std::fs::File;
use std::io::prelude::*;
use std::str;

#[derive(Clone)]
pub struct Object {
    pub data: Vec<u8>,
    pub content_type: Mime,
    pub name: String,
    pub origin: Origin,
    pub scale: u32,
    pub paths: Paths,
    pub retries: u32,
    pub status: Option<StatusCode>,
    pub headers: Option<HeaderMap>,
}
#[derive(Clone)]
pub struct Paths {
    pub base: String,
    pub modified: String,
}
impl Object {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            data: Vec::new(),
            content_type: "text/plain".parse::<Mime>().unwrap(),
            origin: Origin::default(),
            scale: 0,
            paths: Paths {
                base: String::new(),
                modified: String::new(),
            },
            retries: 0,
            status: None,
            headers: None,
        }
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
        self.name = format!("{}-_-{}", self.name, path.replace('/', "-_-"));
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
        self
    }
    pub fn scale(&mut self, scale: u32) -> &mut Self {
        self.scale = scale;
        self
    }
    pub fn get_hash(&self) -> String {
        sha1_smol::Sha1::from(&self.paths.base.as_bytes())
            .digest()
            .to_string()
    }
    pub fn skip(&self) -> Result<HttpResponse> {
        let json = json!({
            "status": 400,
            "error":
                "Max retries reached. Skipping"

        });
        log::error!("Max retries reached for url: {}", self.get_url()?);
        Ok(HttpResponse::BadRequest()
            .content_type("application/json")
            .body(serde_json::to_string(&json)?))
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
        let json = json!({
            "status": 400,
            "error": format!(
                "Object downloaded from {}/{} is not a supported asset",
                self.origin.name,
                self.name
            )
        });
        Ok(HttpResponse::BadRequest()
            .content_type("application/json")
            .body(serde_json::to_string(&json)?))
    }
    pub async fn update_retries(
        &mut self,
        client: &Data<awc::Client>,
        cfg: &Data<AppConfig>,
    ) -> Result<&Self, Box<dyn std::error::Error>> {
        self.retries += 1;
        log::warn!("Updating retries to {}", self.retries);
        let url = Url::parse(&format!("{}/api/{}", cfg.kvstore_uri, self.get_hash()))?;
        let mut res = client
            .post(url.as_str())
            .append_header(("Accept", "application/json"))
            .timeout(Duration::from_secs(cfg.req_timeout))
            .send_json(&json!({"retries": self.retries}))
            .await?;

        if !res.status().is_success() && !res.status().is_client_error() {
            log::warn!(
                "Error while contacting KV store at {} | Response: {:#?}",
                url,
                res
            );
        }
        let data = res.json::<serde_json::Value>().await?;
        self.retries = data["retries"].to_string().parse::<u32>()?;
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
                let data = res.json::<serde_json::Value>().await?;
                self.retries = data["retries"].to_string().parse::<u32>()?;
            }
            StatusCode::INTERNAL_SERVER_ERROR => log::error!("Error contacting kv store"),
            _ => log::warn!("Unexpected response from kv store"),
        };
        Ok(self)
    }
    fn get_url(&self) -> Result<Url> {
        Ok(Url::parse(&format!(
            "{}/{}",
            self.origin.endpoint,
            self.name.replace("-_-", "/")
        ))?)
    }
    pub async fn download(
        &mut self,
        client: &Data<awc::Client>,
        cfg: &Data<AppConfig>,
    ) -> Result<&Self, Box<dyn std::error::Error>> {
        let url = self.get_url()?;
        let start = Instant::now();
        log::info!("Downloading from url: {}", url);
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
                    log::warn!(
                        "{} did not return expected object -- Response: {:#?}",
                        self.get_url()?,
                        r
                    );
                    return Ok(self);
                }
            }
            Err(e) => {
                log::warn!(
                    "Error while connecting to {} | Error: {}",
                    self.get_url()?,
                    e
                );
                return Ok(self);
            }
        };
        self.status = Some(res.status());
        let payload = res.body().limit(cfg.max_body_size_bytes).await?;
        log::debug!(
            "it took {} to download object to memory",
            Elapsed::from(&start)
        );
        self.data = payload.as_ref().to_vec();
        self.headers = Some(res.headers().clone());
        self.content_type = match self.headers.clone().unwrap().get(CONTENT_TYPE) {
            None => {
                log::warn!("The response does not contain a Content-Type header.");
                "application/octet-stream".parse::<mime::Mime>()?
            }
            Some(x) => x.to_str()?.parse::<mime::Mime>()?,
        };
        log::debug!("mime from headers: {}", self.content_type);
        let start = Instant::now();
        let mut file = File::create(&self.paths.base)?;
        file.write_all(&self.data)?;
        log::debug!("it took {} to save object to disk", Elapsed::from(&start));
        Ok(self)
    }
    pub fn process(&self, engine: u32) -> Result<(Mime, Vec<u8>)> {
        let scale = self.scale;
        let mut content_type = self.content_type.clone();
        let data = match self.content_type.as_ref() {
            "image/jpeg" | "image/jpg" => {
                img::scaledown_static(&self.data, scale, ImageFormat::Jpeg)
            }
            "image/png" => match engine {
                1 => img::scaledown_static(&self.data, scale, ImageFormat::Png),
                _ => img::scaledown_png(&self.data, scale),
            },
            "image/webp" => img::scaledown_static(&self.data, scale, ImageFormat::WebP),
            "image/gif" => img::scaledown_gif(&self.paths.base, &self.paths.modified, scale),
            "image/svg+xml" => {
                content_type = mime::IMAGE_PNG;
                img::scaledown_static(&img::svg_to_png(&self.data)?, scale, ImageFormat::Png)
            }
            "video/mp4" => {
                content_type = mime::IMAGE_GIF;
                img::mp4_to_gif(&self.paths.base, &self.paths.modified, scale)
            }
            "application/octet-stream" => {
                log::warn!(
                    "Got unsupported format: {} - Trying to guess format from base.",
                    self.content_type
                );
                //self.guess_content_type(&self.paths.base)?;
                Ok(self.data.clone())
            }
            "application/json" => Ok(self.data.clone()),
            _ => {
                log::warn!(
                    "Got unsupported format: {} - Skipping processing",
                    self.content_type
                );
                Ok(self.data.clone())
            }
        };
        Ok((content_type, data?))
    }
}
