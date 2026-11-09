//! Pairing a phone with this computer over Wi-Fi, as Android Studio does: the phone scans a
//! QR code from Wireless debugging, or the person types the code it shows, and adb pairs with
//! it and connects to it once the phone announces itself on the network.

use std::time::{Duration, Instant};

use super::Android;
use crate::SimResult;

const PAIRING: &str = "_adb-tls-pairing._tcp";
const CONNECT: &str = "_adb-tls-connect._tcp";
const LOOK_EVERY: Duration = Duration::from_secs(1);
/// How long a QR code waits for the phone to scan it.
pub const QR_WAIT: Duration = Duration::from_secs(120);

/// What the phone's camera reads: the name it announces once it has scanned, and the password.
#[derive(Clone, Debug)]
pub struct Invitation {
    pub name: String,
    pub password: String,
}

impl Invitation {
    pub fn new() -> Self {
        let random = |length: usize| uuid::Uuid::new_v4().simple().to_string()[..length].to_owned();
        Self {
            name: format!("sikemux-{}", random(8)),
            password: random(12),
        }
    }

    /// The text the QR code holds, in the form Wireless debugging scans.
    pub fn qr_text(&self) -> String {
        format!("WIFI:T:ADB;S:{};P:{};;", self.name, self.password)
    }
}

impl Android {
    /// The services adb sees announced on the network: name, kind and address.
    async fn announced(&self) -> Vec<(String, String, String)> {
        self.adb(&["mdns", "services"], Duration::from_secs(10))
            .await
            .unwrap_or_default()
            .lines()
            .filter_map(|line| {
                let mut words = line.split_whitespace();
                Some((
                    words.next()?.to_owned(),
                    words.next()?.to_owned(),
                    words.next()?.to_owned(),
                ))
            })
            .collect()
    }

    /// Waits for the phone that scanned `invitation` to announce itself, then pairs and connects.
    pub async fn pair_scanned(&self, invitation: &Invitation) -> SimResult<String> {
        let started = Instant::now();
        loop {
            let found = self
                .announced()
                .await
                .into_iter()
                .find(|(name, kind, _)| *name == invitation.name && kind.starts_with(PAIRING));
            if let Some((_, _, address)) = found {
                return self.pair(&address, &invitation.password).await;
            }
            if started.elapsed() >= QR_WAIT {
                return Err("no phone scanned the code; try again, with the phone on the same network as this computer".into());
            }
            tokio::time::sleep(LOOK_EVERY).await;
        }
    }

    /// Pairs with the phone at `address` using the code it shows, then connects to it.
    pub async fn pair(&self, address: &str, code: &str) -> SimResult<String> {
        let code: String = code.chars().filter(|c| !c.is_whitespace()).collect();
        let answer = self
            .adb(&["pair", address, &code], Duration::from_secs(30))
            .await?;
        if !answer.contains("Successfully paired") {
            return Err(format!("the phone did not pair: {}", answer.trim()));
        }
        let host = address
            .rsplit_once(':')
            .map_or(address, |(host, _)| host)
            .to_owned();
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(30) {
            let connect = self.announced().await.into_iter().find(|(_, kind, at)| {
                kind.starts_with(CONNECT) && at.starts_with(&format!("{host}:"))
            });
            if let Some((_, _, at)) = connect {
                let _ = self.adb(&["connect", &at], Duration::from_secs(15)).await;
            }
            let listed = self.plugged().await;
            if let Some((_, _, model)) = listed
                .into_iter()
                .find(|(serial, state, _)| serial.starts_with(&host) && state == "device")
            {
                return Ok(model);
            }
            tokio::time::sleep(LOOK_EVERY).await;
        }
        Err("the phone paired but did not connect; check Wireless debugging is still on".into())
    }
}
