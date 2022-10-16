use serde_derive::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TwitterProfile {
    #[serde(rename(serialize = "handle"))]
    pub screen_name: Option<String>,
    #[serde(rename(
        serialize = "profile_image_url_lowres",
        deserialize = "profile_image_url_https"
    ))]
    pub avatar_lowres: Option<String>,
    #[serde(rename(
        serialize = "profile_image_url_highres",
        deserialize = "profile_image_url_https"
    ))]
    pub avatar_highres: Option<String>,
    #[serde(rename(serialize = "banner_image_url", deserialize = "profile_banner_url"))]
    pub banner: Option<String>,
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errors: Option<Vec<ApiError>>,
}
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ApiError {
    code: u32,
    message: String,
}
