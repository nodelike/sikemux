//! Asks a core to sign a made-up account registration, as `sikemux login` does once a phone
//! approves, and writes what it runs at and what the core answered to `<out>`. Started
//! elevated, it starts itself again at Medium, as a program the person starts runs.
//!   sign <socket> <out>

use std::path::PathBuf;

use sikemux_client::client::CoreClient;
use sikemux_wire::protocol::ask;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let mut args = std::env::args_os().skip(1).map(PathBuf::from);
    let (Some(socket), Some(out)) = (args.next(), args.next()) else {
        eprintln!("usage: sign <socket> <out>");
        std::process::exit(2);
    };
    #[cfg(windows)]
    if integrity() == "high" {
        return again_at_medium();
    }
    let answer = match CoreClient::connect(&socket).await {
        Ok((client, _events)) => client
            .ask(ask::SignRegistration {
                nonce: "a".repeat(64),
                user_id: "user_test".into(),
            })
            .await
            .map(|signed| format!("signed {}", signed.signature))
            .unwrap_or_else(|error| format!("refused {error}")),
        Err(error) => format!("unreachable {error}"),
    };
    let report = format!("integrity {}\n{answer}\n", integrity());
    std::fs::write(&out, report)
}

#[cfg(windows)]
fn integrity() -> &'static str {
    match sikemux_trust::Token::own_integrity() {
        Ok(sikemux_trust::Integrity::Medium) => "medium",
        Ok(sikemux_trust::Integrity::High) => "high",
        Err(_) => "unknown",
    }
}

#[cfg(windows)]
fn again_at_medium() -> std::io::Result<()> {
    let token = sikemux_trust::Token::own().and_then(|own| own.for_child())?;
    let environment: Vec<_> = std::env::vars_os().collect();
    let command_line = std::env::args()
        .map(|arg| format!("\"{arg}\""))
        .collect::<Vec<_>>()
        .join(" ");
    sikemux_trust::spawn(&sikemux_trust::Start {
        token: &token,
        command_line: &command_line,
        inherit: &[],
        output: None,
        environment: Some(&environment),
        directory: None,
        process_access: None,
        on_desktop: false,
    })?;
    Ok(())
}

#[cfg(not(windows))]
fn integrity() -> &'static str {
    "unknown"
}
