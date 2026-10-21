use sikemux_plugin_api::{Service, ServiceError, Unreachable};

pub struct Rundeck;

impl Service for Rundeck {
    const ID: &'static str = "rundeck";
    const NAME: &'static str = "Rundeck";

    fn unreachable(kind: Unreachable, host: Option<&str>) -> String {
        let how = match kind {
            Unreachable::Timeout => "it did not answer in time",
            Unreachable::Offline => "the connection failed",
        };
        format!(
            "Can't reach Rundeck at {} ({how}). Check the network or VPN.",
            host.unwrap_or("its address")
        )
    }
}

pub type RundeckError = ServiceError<Rundeck>;
pub type RundeckResult<T> = Result<T, RundeckError>;
