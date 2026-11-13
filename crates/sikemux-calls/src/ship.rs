use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
pub struct WebWriteParams {
    pub agent: String,
    pub project: String,
    pub what: String,
    pub origin: String,
}

#[derive(Deserialize, Serialize)]
pub struct HoldParams {
    pub hold: String,
}
