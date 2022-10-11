use crate::config::{AppConfig, Origin};
use crate::img;
use crate::utils::{self, Elapsed};
use crate::CONTENT_TYPE;
use crate::{web::Data, HeaderMap, StatusCode};
use crate::{Duration, Instant};
use anyhow::Result;
use image::ImageFormat;
use mime::Mime;
use std::fs;
use std::fs::File;
use std::io::prelude::*;

#[derive(Clone)]
pub struct Object {
    pub data: Vec<u8>,
    pub content_type: Mime,
    pub name: String,
    pub origin: Origin,
    pub scale: u32,
    pub paths: Paths,
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
            status: None,
            headers: None,
        }
    }
    pub fn create_dir(&self, path: &str) -> Result<()> {
        fs::create_dir_all(format!("{}/base/{}", path, self.origin.name))?;
        fs::create_dir_all(format!("{}/mod/{}/{}", path, self.origin.name, self.scale))?;
        Ok(())
    }
    pub fn try_open(&mut self) -> Result<&Self, Box<dyn std::error::Error>> {
        self.data = if std::path::Path::new(&self.paths.base).exists() && !std::path::Path::new(&self.paths.modified).exists() {
            self.content_type = utils::guess_content_type(&self.paths.base)?;
            utils::read_from_file(&self.paths.base)?
        } else if std::path::Path::new(&self.paths.modified).exists() && self.scale != 0 {
            self.content_type = utils::guess_content_type(&self.paths.modified)?;
            utils::read_from_file(&self.paths.modified)?
        } else {
            vec![]
        };
        self.status = Some(StatusCode::OK);
        Ok(self)
    }

    pub fn rename(&mut self, path: &str) -> &mut Self {
        self.name = format!("{}-_-{}", self.name, path.replace("/", "-_-"));
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
    pub async fn download(
        &mut self,
        client: &Data<awc::Client>,
        cfg: &Data<AppConfig>,
    ) -> Result<&Self, Box<dyn std::error::Error>> {
        let url = format!("{}/{}", self.origin.endpoint, self.name.replace("-_-", "/"));
        let start = Instant::now();
        log::info!("Downloading object from: {}", url);

        let mut res = client
            .get(&url)
            .timeout(Duration::from_secs(cfg.req_timeout))
            .send()
            .await?;

        if !res.status().is_success() {
            log::warn!(
                "{} did not return expected object: {} -- Response: {:#?}",
                self.origin.name,
                self.name,
                res
            );
        }
        self.status = Some(res.status());
        let payload = res
            .body()
            // expected object is larger than default body limit
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
                "application/octet-stream".parse::<mime::Mime>()?
            }
            Some(x) => x.to_str()?.parse::<mime::Mime>()?,
        };
        log::info!("got mime from headers: {}", self.content_type);
        let start = Instant::now();

        let mut file = File::create(&self.paths.base)?;
        file.write_all(&self.data)?;
        log::info!("it took {} to save object to disk", Elapsed::from(&start));
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
