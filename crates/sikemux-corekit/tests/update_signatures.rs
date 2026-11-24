//! An update installs only when the release key signed it as that update. The key and signatures
//! here were made for this test and sign nothing else.
#![cfg(unix)]
#![allow(clippy::unwrap_used)]

use std::io::{Read, Write};
use std::net::TcpListener;

use base64::Engine;
use minisign_verify::PublicKey;
use semver::Version;
use sikemux_corekit::release_feed::{fetch_verified, Asset};
use sikemux_release::{Channel, Product, Update};

const KEY: &str = "RWQ0xv5f7jp/edUQ4hMZ17LxJnjbcRTmRXJybFroWV/CtdZTUndTOZNH";
const BUILD: &[u8] = b"a build";
const SIGNATURE: &str =
    "RUQ0xv5f7jp/eVkICnO44auaGCZAkomeC778ORNPIG3uoi4BPur5VlstYL/LEhFGO7YONwRv8gthYA1IXbkzR6jNp569xRPv8gQ=";
const OLDER: (&str, &str) = (
    "timestamp:0\tfile:Sikemux.app.tar.gz\tversion:0.6.2",
    "pKJCamkt5iYm4YR4Wm5xR6kLlW5E+l30WEGTVS7iwSzWc2QwQXB8O9CU/wrLdIBzmSvq1venC3gmXorQHbcnDQ==",
);
const NEWER: (&str, &str) = (
    "timestamp:0\tfile:Sikemux.app.tar.gz\tversion:0.6.3",
    "XMVwMbsoir0a5X4wseZiqw1EwkQJmGRcBfUKHaHLxCVTHqRFT6eerEuSZWsrqsTD7+EnXNFwbllqsFbOyJDLDg==",
);
const NIGHTLY: (&str, &str) = (
    "timestamp:0\tfile:Sikemux.app.tar.gz\tversion:0.6.4-nightly.1",
    "Rd7xm7QeM/wMUpXRzuCCG7nX5nb11cuMTn/6XliqsMl56BE12VM0FMOlr/A4trQoHu83Piz9fCtGOdgiS9rIBA==",
);
const UNNAMED: (&str, &str) = (
    "timestamp:0\tfile:Sikemux.app.tar.gz",
    "2VdvKUzo1RF2agHfU7SNtw5h/YFey18YgfpaXI1yuJ33wCLh5WpmvUqudlTn+44i7ZQWtTo9YQau5VbiCaKXDA==",
);

async fn offer(
    claimed: &str,
    running: &str,
    channel: Channel,
    product: Product,
    (comment, over_comment): (&str, &str),
) -> Result<(), String> {
    let server = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/build", server.local_addr().unwrap());
    std::thread::spawn(move || {
        let (mut asked, _) = server.accept().unwrap();
        let _ = asked.read(&mut [0; 4096]);
        let head = format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n", BUILD.len());
        asked.write_all(&[head.as_bytes(), BUILD].concat()).unwrap();
    });
    let signature = base64::engine::general_purpose::STANDARD.encode(format!(
        "untrusted comment: test\n{SIGNATURE}\ntrusted comment: {comment}\n{over_comment}\n"
    ));
    let update = Update {
        product,
        version: Version::parse(claimed).unwrap(),
        running: Version::parse(running).unwrap(),
        channel,
    };
    let folder = tempfile::tempdir().unwrap();
    fetch_verified(
        &update,
        &Asset { signature, url },
        &PublicKey::from_base64(KEY).unwrap(),
        &folder.path().join("build"),
        |_, _| {},
    )
    .await
}

#[tokio::test]
async fn an_older_signed_build_is_not_an_update() {
    use Channel::{Nightly, Stable};
    use Product::{AppImage, MacApp};
    let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();

    for (claimed, running, channel, signed) in [
        ("0.6.3", "0.6.2", Stable, NEWER),
        ("0.6.3", "0.6.3-nightly.9", Nightly, NEWER),
        ("0.6.4-nightly.1", "0.6.3", Nightly, NIGHTLY),
    ] {
        let offered = offer(claimed, running, channel, MacApp, signed).await;
        assert_eq!(offered, Ok(()), "{claimed} over {running}");
    }

    for (claimed, running, channel, product, signed) in [
        // An older build the feed calls newer.
        ("0.6.4", "0.6.3", Stable, MacApp, OLDER),
        // An older build under its own version, and the running one again.
        ("0.6.2", "0.6.3", Stable, MacApp, OLDER),
        ("0.6.3", "0.6.3", Stable, MacApp, NEWER),
        // A nightly on stable, as itself and under a stable version.
        ("0.6.4-nightly.1", "0.6.3", Stable, MacApp, NIGHTLY),
        ("0.6.4", "0.6.3", Stable, MacApp, NIGHTLY),
        // The Mac's file where an AppImage belongs.
        ("0.6.3", "0.6.2", Stable, AppImage, NEWER),
        // No version signed, and a signed version altered.
        ("0.6.3", "0.6.2", Stable, MacApp, UNNAMED),
        ("0.6.3", "0.6.2", Stable, MacApp, (NEWER.0, OLDER.1)),
    ] {
        let offered = offer(claimed, running, channel, product, signed).await;
        assert!(
            offered.is_err(),
            "{claimed} over {running} {product:?} {signed:?}"
        );
    }
}
