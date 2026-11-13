use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
pub struct NewProjectParams {
    pub name: String,
}

#[derive(Deserialize, Serialize)]
pub struct CloneProjectParams {
    pub url: String,
}
