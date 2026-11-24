//! What a release's signature must say before anything installs it as an update. Every file a
//! release ships is signed with a comment naming that file and its version, and the signature
//! covers the comment. A feed is not signed, so nothing it says about a file is believed until
//! the comment says the same.

use base64::Engine;
use minisign_verify::{PublicKey, Signature, StreamVerifier};
use semver::Version;

/// Larger than any file a release ships.
pub const LARGEST_FILE: u64 = 512 << 20;

/// What an update replaces on this machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Product {
    MacApp,
    AppImage,
    Installer,
    HostCore,
}

impl Product {
    /// The file a release ships this as, for this machine's architecture.
    pub fn file(self, version: &Version) -> String {
        let arch = std::env::consts::ARCH;
        match self {
            Self::MacApp => "Sikemux.app.tar.gz".into(),
            Self::AppImage => {
                let arch = match arch {
                    "x86_64" => "amd64",
                    "aarch64" => "arm64",
                    other => other,
                };
                format!("Sikemux_{version}_{arch}.AppImage")
            }
            Self::Installer => {
                let arch = match arch {
                    "x86_64" => "x64",
                    "aarch64" => "arm64",
                    other => other,
                };
                format!("Sikemux_{version}_{arch}-setup.exe")
            }
            Self::HostCore => format!("sikemux-host-{version}-linux-{arch}.tar.gz"),
        }
    }
}

/// Which releases a machine takes: stable ones alone, or nightlies and stable ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    Stable,
    Nightly,
}

impl Channel {
    fn takes(self, version: &Version) -> bool {
        version.pre.is_empty()
            || (self == Self::Nightly && version.pre.as_str().starts_with("nightly."))
    }
}

/// An update a feed offers, and what it would replace.
#[derive(Clone, Debug)]
pub struct Update {
    pub product: Product,
    pub version: Version,
    pub running: Version,
    pub channel: Channel,
}

impl Update {
    /// The file this update is, as a release names it.
    pub fn file(&self) -> String {
        self.product.file(&self.version)
    }

    /// The release's signature, once it says it was made for this update. What it says counts
    /// only after the file has been checked against it with `check`.
    pub fn signature(&self, encoded: &str) -> Result<UpdateSignature, String> {
        let unreadable = |error: String| format!("the release's signature: {error}");
        let text = base64::engine::general_purpose::STANDARD
            .decode(encoded.trim())
            .map_err(|error| unreadable(error.to_string()))?;
        let signature = Signature::decode(&String::from_utf8_lossy(&text))
            .map_err(|error| unreadable(error.to_string()))?;
        self.allows(signature.trusted_comment())?;
        Ok(UpdateSignature(signature))
    }

    fn allows(&self, comment: &str) -> Result<(), String> {
        let Self {
            version, running, ..
        } = self;
        if version <= running {
            return Err(format!("Sikemux {running} is already as new as {version}"));
        }
        if !self.channel.takes(version) {
            return Err(format!(
                "Sikemux {version} is not a release of this channel"
            ));
        }
        let file = self.file();
        let says = |field: String| comment.split('\t').any(|said| said == field);
        if says(format!("file:{file}")) && says(format!("version:{version}")) {
            return Ok(());
        }
        Err(format!(
            "the signature of {file} was not made for Sikemux {version} on this machine ({comment})"
        ))
    }
}

/// A signature made for one update, whose file has yet to be checked against it.
pub struct UpdateSignature(Signature);

impl UpdateSignature {
    pub fn check<'a>(&'a self, key: &'a PublicKey) -> Result<Check<'a>, String> {
        key.verify_stream(&self.0)
            .map(Check)
            .map_err(|_| "the release is signed with another key".to_owned())
    }
}

/// A file being checked against its signature as it comes.
pub struct Check<'a>(StreamVerifier<'a>);

impl Check<'_> {
    pub fn update(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }

    pub fn finish(mut self) -> Result<(), String> {
        self.0
            .finalize()
            .map_err(|_| "the download does not match the release's signature".to_owned())
    }
}
