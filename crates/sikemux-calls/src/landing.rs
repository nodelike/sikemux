use serde::{Deserialize, Serialize};
use sikemux_view::turn::Landed;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LandedMeansParams {
    pub project: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetLandedMeansParams {
    pub project: String,
    /// None goes back to what the project's history says.
    #[serde(default)]
    pub means: Option<Landed>,
}
