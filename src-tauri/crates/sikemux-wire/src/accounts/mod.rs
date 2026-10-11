//! What the core and the phone share with the accounts server at
//! api.sikemux.com, generated from `server/protocol/schema`.

pub mod protocol;

#[cfg(test)]
type RoundTrip = serde_json::Result<serde_json::Value>;

#[cfg(test)]
fn through<T: serde::de::DeserializeOwned + serde::Serialize>(json: &str) -> RoundTrip {
    serde_json::to_value(serde_json::from_str::<T>(json)?)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    /// Every example in `server/protocol/fixtures` reads as its Rust type and
    /// writes back unchanged, so the server and the core agree on the wire.
    #[test]
    fn fixtures_round_trip() -> Result<(), Box<dyn std::error::Error>> {
        let fixtures =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../server/protocol/fixtures");
        let mut checked = 0;
        for kind in std::fs::read_dir(&fixtures)? {
            let kind = kind?;
            let name = kind.file_name().to_string_lossy().into_owned();
            for example in std::fs::read_dir(kind.path())? {
                let path = example?.path();
                let json = std::fs::read_to_string(&path)?;
                let written = super::protocol::round_trip(&name, &json)
                    .ok_or_else(|| format!("{name} has fixtures but no Rust type"))?
                    .map_err(|error| format!("{}: {error}", path.display()))?;
                let read: serde_json::Value = serde_json::from_str(&json)?;
                assert_eq!(
                    written,
                    read,
                    "{} changed on the way through Rust",
                    path.display()
                );
                checked += 1;
            }
        }
        assert!(checked > 0, "no fixtures under {}", fixtures.display());
        Ok(())
    }

    #[test]
    fn unknown_enum_values_still_read() -> serde_json::Result<()> {
        let code: super::protocol::ErrorCode = serde_json::from_str("\"added_later\"")?;
        assert_eq!(code, super::protocol::ErrorCode::Unknown);
        Ok(())
    }
}
