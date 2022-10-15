use serde_derive::{Deserialize, Serialize};
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TwitterProfile {
    pub handle: String,
    pub profile_image_url_lowres: String,
    pub profile_image_url_highres: String,
    pub banner_image_url: String,
    pub description: String,
}

impl TwitterProfile {
    pub fn build(h: serde_json::Value) -> Self {
        let image_url = &h[0]["profile_image_url_https"];
        Self {
            handle: h[0]["screen_name"].as_str().unwrap().to_string(),
            profile_image_url_lowres: image_url.as_str().unwrap().to_string(),
            profile_image_url_highres: image_url
                .as_str()
                .unwrap()
                .to_string()
                .replace("_normal", ""),
            banner_image_url: h[0]["profile_banner_url"].as_str().unwrap().to_string(),
            description: h[0]["description"].as_str().unwrap().to_string(),
        }
    }
}
