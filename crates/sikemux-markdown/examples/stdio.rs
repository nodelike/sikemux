//! Parses one JSON request per line of stdin and answers with one line of JSON,
//! for tools that draw the app outside it, such as the showcase.

use std::io::{self, BufRead, Write};

use serde::Deserialize;
use sikemux_markdown::{parse, Options};

#[derive(Deserialize)]
struct Request {
    text: String,
    #[serde(default)]
    options: Options,
    #[serde(default)]
    skip: usize,
}

fn main() -> io::Result<()> {
    let mut out = io::stdout().lock();
    for line in io::stdin().lock().lines() {
        let request: Request = serde_json::from_str(&line?).map_err(io::Error::other)?;
        let blocks = parse(&request.text, request.options, request.skip);
        serde_json::to_writer(&mut out, &blocks).map_err(io::Error::other)?;
        out.write_all(b"\n")?;
        out.flush()?;
    }
    Ok(())
}
