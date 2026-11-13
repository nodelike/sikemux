use serde::{Deserialize, Serialize};

/// What a call that takes nothing is sent; whatever a client sends reads as nothing.
#[derive(Serialize)]
pub struct NoParams {}

impl<'de> Deserialize<'de> for NoParams {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        serde::de::IgnoredAny::deserialize(deserializer)?;
        Ok(NoParams {})
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StopBuildParams {
    pub id: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoFirstParams {
    pub id: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentParams {
    pub agent_id: String,
}

/// A field the params name, null or not, as against one they leave out.
pub fn given<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    field: D,
) -> Result<Option<Option<T>>, D::Error> {
    Option::deserialize(field).map(Some)
}
