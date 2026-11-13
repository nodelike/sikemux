use serde::{Deserialize, Serialize};
use sikemux_view::turn::TurnStep;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TakeStepParams {
    pub agent_id: String,
    pub step: TurnStep,
}
