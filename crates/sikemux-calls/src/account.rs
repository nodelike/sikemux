#[derive(serde::Deserialize, serde::Serialize)]
pub struct DeviceParams {
    pub key: String,
    #[serde(default)]
    pub name: Option<String>,
}
