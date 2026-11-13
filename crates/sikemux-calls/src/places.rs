use serde::{Deserialize, Serialize};
use sikemux_view::places::Level;
use sikemux_view::places::Recognise;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlacesParams {
    pub project: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaceLevelParams {
    pub project: String,
    pub place: String,
    pub level: Level,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddPlaceParams {
    pub project: String,
    pub name: String,
    pub production: bool,
    pub recognise: Vec<Recognise>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeenParams {
    pub project: String,
    pub seen: String,
}
