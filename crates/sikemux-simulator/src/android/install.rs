//! Sets up the Android Emulator on a computer with no Android Studio: the SDK's
//! platform-tools, the emulator and one Google Play system image, downloaded from
//! Google's own repository as sdkmanager would, so no Java is needed, and a virtual
//! device to run it on. An SDK already here is reused and only what it lacks is added.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use sha1::{Digest, Sha1};

use super::sdk;
use crate::SimResult;

const REPOSITORY: &str = "https://dl.google.com/android/repository/";
const PACKAGES: &str = "repository2-3.xml";
const IMAGES: &str = "sys-img/google_apis_playstore/sys-img2-3.xml";
const STABLE: &str = "channel-0";
const TAG: &str = "google_apis_playstore";
/// The same image built for 16 KB pages, which Android Studio picks for new devices; the 4 KB
/// build of Android 17 never lets adb in.
const TAG_16K: &str = "google_apis_playstore_ps16k";
const DEVICE_ID: &str = "Sikemux_Pixel";
pub const DEVICE_NAME: &str = "Pixel";

/// One package to download and where it unpacks.
#[derive(Clone, Debug)]
pub struct Package {
    pub name: String,
    url: String,
    sha1: String,
    pub size: u64,
    /// Where the archive's one top folder goes, under the SDK.
    folder: PathBuf,
}

/// What setting up would do: the SDK it fills, the packages it lacks, and whether a virtual device is made.
#[derive(Clone, Debug)]
pub struct Plan {
    pub root: PathBuf,
    avd_home: PathBuf,
    pub packages: Vec<Package>,
    image: Option<(String, String)>,
    pub device: bool,
}

impl Plan {
    pub fn size(&self) -> u64 {
        self.packages.iter().map(|package| package.size).sum()
    }

    pub fn summary(&self) -> Value {
        json!({
            "root": self.root,
            "bytes": self.size(),
            "packages": self.packages.iter().map(|package| &package.name).collect::<Vec<_>>(),
            "device": self.device,
        })
    }
}

fn host_os() -> &'static str {
    if cfg!(target_os = "macos") {
        "macosx"
    } else if cfg!(windows) {
        "windows"
    } else {
        "linux"
    }
}

fn host_abi() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "arm64-v8a"
    } else {
        "x86_64"
    }
}

async fn fetch_text(url: &str) -> SimResult<String> {
    let client = sikemux_base::http::client().map_err(|error| error.to_string())?;
    let response = client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| format!("could not reach Google's Android repository: {error}"))?;
    response.text().await.map_err(|error| error.to_string())
}

/// The text between `<tag>` and `</tag>` in `body`, the first of them.
fn tag<'a>(body: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&format!("</{name}>"))? + start;
    Some(body[start..end].trim())
}

/// Each `<remotePackage>` in a repository listing: its path and body.
fn remote_packages(xml: &str) -> Vec<(&str, &str)> {
    xml.split("<remotePackage path=\"")
        .skip(1)
        .filter_map(|chunk| {
            let (path, rest) = chunk.split_once('"')?;
            let body = rest.split("</remotePackage>").next()?;
            Some((path, body))
        })
        .collect()
}

fn stable(body: &str) -> bool {
    body.contains(&format!("<channelRef ref=\"{STABLE}\"/>"))
}

fn revision(body: &str) -> Vec<u64> {
    let Some(revision) = tag(body, "revision") else {
        return Vec::new();
    };
    ["major", "minor", "micro"]
        .iter()
        .map(|part| {
            tag(revision, part)
                .and_then(|n| n.parse().ok())
                .unwrap_or(0)
        })
        .collect()
}

/// The archive in `body` for this computer, or the one archive a system image has.
fn archive(body: &str) -> Option<(String, String, u64)> {
    body.split("<archive>").skip(1).find_map(|archive| {
        let os = tag(archive, "host-os");
        let arch = tag(archive, "host-arch");
        let fits = os.is_none_or(|os| os == host_os())
            && arch.is_none_or(|arch| match host_os() {
                "macosx" if cfg!(target_arch = "aarch64") => arch == "aarch64",
                _ => arch == "x64",
            });
        if !fits {
            return None;
        }
        let complete = tag(archive, "complete")?;
        Some((
            tag(complete, "url")?.to_owned(),
            checksum(complete)?,
            tag(complete, "size")?.parse().ok()?,
        ))
    })
}

fn checksum(complete: &str) -> Option<String> {
    let start = complete.find("<checksum")?;
    let value = &complete[start..];
    let value = &value[value.find('>')? + 1..];
    Some(value[..value.find("</checksum>")?].trim().to_owned())
}

/// The newest stable package at `path` with an archive for this computer.
fn newest(xml: &str, path: &str) -> Option<(String, String, u64)> {
    remote_packages(xml)
        .into_iter()
        .filter(|(at, body)| *at == path && stable(body))
        .filter_map(|(_, body)| Some((revision(body), archive(body)?)))
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, archive)| archive)
}

/// The newest stable Google Play image for this computer, the 16 KB build where there is one:
/// its API folder name, its tag and its archive.
fn newest_image(xml: &str) -> Option<(String, String, (String, String, u64))> {
    remote_packages(xml)
        .into_iter()
        .filter(|(_, body)| stable(body))
        .filter_map(|(path, body)| {
            let mut parts = path.split(';');
            let (Some("system-images"), Some(api), Some(tag_id), Some(abi)) =
                (parts.next(), parts.next(), parts.next(), parts.next())
            else {
                return None;
            };
            let level = api.strip_prefix("android-")?;
            let number: Vec<u64> = level
                .split('.')
                .map(|part| part.parse().ok())
                .collect::<Option<_>>()?;
            let wanted = tag_id == TAG || tag_id == TAG_16K;
            (wanted && abi == host_abi()).then_some((
                (number, tag_id == TAG_16K),
                api.to_owned(),
                tag_id.to_owned(),
                archive(body)?,
            ))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, api, tag, archive)| (api, tag, archive))
}

/// The image an existing virtual device already runs, so none is downloaded for it.
fn installed_image(root: &Path) -> Option<String> {
    let images = root.join("system-images");
    std::fs::read_dir(images)
        .ok()?
        .flatten()
        .find(|api| {
            std::fs::read_dir(api.path())
                .into_iter()
                .flatten()
                .flatten()
                .any(|tag| tag.path().join(host_abi()).join("system.img").is_file())
        })
        .and_then(|api| api.file_name().into_string().ok())
}

/// Works out what setting up an SDK at `root`, with virtual devices kept in `avd_home`,
/// needs, asking Google's repository for the newest packages.
pub async fn plan(root: PathBuf, avd_home: PathBuf) -> SimResult<Plan> {
    let (packages_url, images_url) = (
        format!("{REPOSITORY}{PACKAGES}"),
        format!("{REPOSITORY}{IMAGES}"),
    );
    let (packages_xml, images_xml) =
        tokio::try_join!(fetch_text(&packages_url), fetch_text(&images_url))?;
    let mut packages = Vec::new();
    for (path, name) in [
        ("platform-tools", "Platform tools"),
        ("emulator", "Emulator"),
    ] {
        if sdk::tool(
            &root,
            path,
            if path == "emulator" {
                "emulator"
            } else {
                "adb"
            },
        )
        .is_file()
        {
            continue;
        }
        let (url, sha1, size) = newest(&packages_xml, path)
            .ok_or_else(|| format!("Google's repository lists no {name} for this computer"))?;
        packages.push(Package {
            name: name.into(),
            url: format!("{REPOSITORY}{url}"),
            sha1,
            size,
            folder: PathBuf::from(path),
        });
    }
    let no_devices = std::fs::read_dir(&avd_home).map_or(true, |entries| {
        !entries
            .flatten()
            .any(|entry| entry.path().extension().is_some_and(|ext| ext == "ini"))
    });
    let have_image = installed_image(&root);
    let image = match &have_image {
        Some(api) => Some((api.clone(), String::new())),
        None => {
            let (api, tag, (url, sha1, size)) = newest_image(&images_xml)
                .ok_or("Google's repository lists no Google Play system image for this computer")?;
            let version = super::android_version(api.trim_start_matches("android-"));
            packages.push(Package {
                name: format!("{version} system image"),
                url: format!("{REPOSITORY}sys-img/{TAG}/{url}"),
                sha1,
                size,
                folder: PathBuf::from("system-images")
                    .join(&api)
                    .join(&tag)
                    .join(host_abi()),
            });
            Some((api, tag))
        }
    };
    Ok(Plan {
        root,
        avd_home,
        device: no_devices,
        image,
        packages,
    })
}

/// How far setting up has come: the step, and the share of all its downloading done.
pub struct Progress<'a> {
    pub step: &'a str,
    pub fraction: f64,
}

/// Downloads and unpacks what `plan` lacks and makes the virtual device.
pub async fn install(plan: &Plan, mut progress: impl FnMut(Progress)) -> SimResult<()> {
    let total = plan.size().max(1);
    let mut done = 0;
    std::fs::create_dir_all(plan.root.join("platforms")).map_err(|error| error.to_string())?;
    let downloads = plan.root.join(".downloads");
    std::fs::create_dir_all(&downloads).map_err(|error| error.to_string())?;
    for package in &plan.packages {
        let zip = downloads.join(format!("{}.zip", package.sha1));
        download(package, &zip, |bytes| {
            progress(Progress {
                step: &package.name,
                fraction: (done + bytes) as f64 / total as f64,
            })
        })
        .await?;
        progress(Progress {
            step: &format!("Unpacking the {}", package.name),
            fraction: (done + package.size) as f64 / total as f64,
        });
        let (zip_at, root, folder) = (zip.clone(), plan.root.clone(), package.folder.clone());
        tokio::task::spawn_blocking(move || unpack(&zip_at, &root.join(folder)))
            .await
            .map_err(|error| error.to_string())??;
        let _ = std::fs::remove_file(&zip);
        done += package.size;
    }
    let _ = std::fs::remove_dir(&downloads);
    if plan.device {
        if let Some((api, tag)) = &plan.image {
            progress(Progress {
                step: "Making the Pixel",
                fraction: 1.,
            });
            make_device(&plan.root, &plan.avd_home, api, tag)?;
        }
    }
    Ok(())
}

/// Downloads `package` to `zip`, carrying on from a download an earlier try left, and checks it.
async fn download(package: &Package, zip: &Path, mut progress: impl FnMut(u64)) -> SimResult<()> {
    let partial = zip.with_extension("partial");
    let kept = std::fs::metadata(&partial).map_or(0, |meta| meta.len());
    let client = sikemux_base::http::client().map_err(|error| error.to_string())?;
    let mut request = client.get(&package.url);
    if kept > 0 && kept < package.size {
        request = request.header(reqwest::header::RANGE, format!("bytes={kept}-"));
    }
    let mut response = request
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| format!("could not download {}: {error}", package.name))?;
    let resumed = kept > 0 && response.status() == reqwest::StatusCode::PARTIAL_CONTENT;
    let mut hasher = Sha1::new();
    let mut written = 0;
    let mut out = if resumed {
        let mut earlier = std::fs::File::open(&partial).map_err(|error| error.to_string())?;
        let mut buffer = vec![0; 1 << 20];
        loop {
            let read = earlier
                .read(&mut buffer)
                .map_err(|error| error.to_string())?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        written = kept;
        std::fs::OpenOptions::new()
            .append(true)
            .open(&partial)
            .map_err(|error| error.to_string())?
    } else {
        std::fs::File::create(&partial).map_err(|error| error.to_string())?
    };
    progress(written);
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("the download of {} stopped: {error}", package.name))?
    {
        hasher.update(&chunk);
        out.write_all(&chunk).map_err(|error| error.to_string())?;
        written += chunk.len() as u64;
        progress(written);
    }
    out.sync_all().map_err(|error| error.to_string())?;
    let hash: String = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if hash != package.sha1 {
        let _ = std::fs::remove_file(&partial);
        return Err(format!(
            "{} did not download whole; try again",
            package.name
        ));
    }
    std::fs::rename(&partial, zip).map_err(|error| error.to_string())
}

/// Unpacks an archive whose one top folder becomes `destination`.
fn unpack(zip: &Path, destination: &Path) -> SimResult<()> {
    let parent = destination.parent().ok_or("nowhere to unpack")?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let scratch = parent.join(format!(
        ".unpacking-{}",
        destination
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("package")
    ));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).map_err(|error| error.to_string())?;
    let mut command = if cfg!(windows) {
        let mut tar = sikemux_process::user_environment::command("tar");
        tar.arg("-xf").arg(zip).arg("-C").arg(&scratch);
        tar
    } else {
        let mut unzip = sikemux_process::user_environment::command("unzip");
        unzip.args(["-q", "-o"]).arg(zip).arg("-d").arg(&scratch);
        unzip
    };
    let status = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|error| format!("could not unpack {}: {error}", zip.display()))?;
    if !status.success() {
        let _ = std::fs::remove_dir_all(&scratch);
        return Err(format!("could not unpack {}", zip.display()));
    }
    let top: Vec<PathBuf> = std::fs::read_dir(&scratch)
        .map_err(|error| error.to_string())?
        .flatten()
        .map(|entry| entry.path())
        .collect();
    let [top] = top.as_slice() else {
        let _ = std::fs::remove_dir_all(&scratch);
        return Err(format!("{} did not hold one folder", zip.display()));
    };
    if destination.exists() {
        let _ = std::fs::remove_dir_all(&scratch);
        return Err(format!("{} is already there", destination.display()));
    }
    std::fs::rename(top, destination).map_err(|error| error.to_string())?;
    let _ = std::fs::remove_dir_all(&scratch);
    Ok(())
}

/// Makes a phone-sized virtual device on the image, as Android Studio's Device Manager would.
fn make_device(root: &Path, home: &Path, api: &str, tag: &str) -> SimResult<()> {
    let folder = home.join(format!("{DEVICE_ID}.avd"));
    std::fs::create_dir_all(&folder).map_err(|error| error.to_string())?;
    let tag = if tag.is_empty() {
        std::fs::read_dir(root.join("system-images").join(api))
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .find(|entry| entry.path().join(host_abi()).join("system.img").is_file())
            .and_then(|entry| entry.file_name().into_string().ok())
            .ok_or("the system image is missing")?
    } else {
        tag.to_owned()
    };
    let arch = if host_abi() == "arm64-v8a" {
        "arm64"
    } else {
        "x86_64"
    };
    let play = tag.contains("playstore");
    let config = format!(
        "AvdId={DEVICE_ID}\n\
         avd.ini.displayname={DEVICE_NAME}\n\
         avd.ini.encoding=UTF-8\n\
         abi.type={abi}\n\
         hw.cpu.arch={arch}\n\
         hw.cpu.ncore=4\n\
         image.sysdir.1=system-images/{api}/{tag}/{abi}/\n\
         tag.id={TAG}\n\
         PlayStore.enabled={play}\n\
         target={api}\n\
         hw.lcd.width=1080\n\
         hw.lcd.height=2400\n\
         hw.lcd.density=420\n\
         hw.ramSize=2048\n\
         disk.dataPartition.size=6G\n\
         hw.keyboard=yes\n\
         hw.gpu.enabled=yes\n\
         hw.gpu.mode=auto\n\
         hw.initialOrientation=portrait\n\
         fastboot.forceFastBoot=yes\n\
         showDeviceFrame=no\n",
        abi = host_abi(),
    );
    std::fs::write(folder.join("config.ini"), config).map_err(|error| error.to_string())?;
    let pointer = format!(
        "avd.ini.encoding=UTF-8\npath={}\npath.rel=avd/{DEVICE_ID}.avd\ntarget={api}\n",
        folder.display()
    );
    std::fs::write(home.join(format!("{DEVICE_ID}.ini")), pointer)
        .map_err(|error| error.to_string())
}
