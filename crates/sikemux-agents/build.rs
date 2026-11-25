use std::error::Error;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-changed=models-dev.json");
    let raw = std::fs::read("models-dev.json")?;
    let compressed = zstd::stream::encode_all(raw.as_slice(), 15)?;
    let out = PathBuf::from(std::env::var("OUT_DIR")?).join("models-dev.json.zst");
    std::fs::write(out, compressed)?;
    Ok(())
}
