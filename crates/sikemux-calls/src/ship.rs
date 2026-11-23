use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
pub struct WebWriteParams {
    pub agent: String,
    pub project: String,
    pub what: String,
    pub origin: String,
    /// `what` with each address's query, read only to tell where the write lands.
    pub with_queries: String,
}

#[derive(Deserialize, Serialize)]
pub struct HoldParams {
    pub hold: String,
}
