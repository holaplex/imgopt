use serde::Serialize;
pub mod admin;
pub mod public;
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    status: u32,
    error: String,
}
impl ErrorResponse {
    pub fn new(status: u32, error: &str) -> Self {
        Self {
            status,
            error: error.to_string(),
        }
    }
}
