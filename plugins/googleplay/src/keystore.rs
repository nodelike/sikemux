// Upload keystores: made or adopted here, their passwords kept in the Keychain and
// handed to the JDK's keytool and jarsigner, or the SDK's apksigner, only through
// an environment variable, so no password is ever on a command line or in an answer.

use std::path::{Path, PathBuf};
use std::process::Command;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use ring::rand::{SecureRandom, SystemRandom};
use serde::Deserialize;
use serde_json::{json, Value};
use sikemux_plugin_api::store;
use sikemux_process::user_environment::{self, var_os};

use crate::auth::expand;
use crate::config::{self, keystore_entry, Keystore, PlayConfig, KEYCHAIN};
use crate::error::{PlayError, PlayResult};

const PASSWORD_VARIABLE: &str = "SIKEMUX_KEYSTORE_PASSWORD";
const MAX_PASSWORD_FILE: u64 = 4096;

fn java_home() -> Option<PathBuf> {
    let from_variable = var_os("JAVA_HOME").map(PathBuf::from);
    let from_macos = user_environment::command("/usr/libexec/java_home")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()));
    let android_studio =
        PathBuf::from("/Applications/Android Studio.app/Contents/jbr/Contents/Home");
    [from_variable, from_macos, Some(android_studio)]
        .into_iter()
        .flatten()
        .find(|home| home.join("bin/keytool").is_file())
}

fn java_tool(name: &str) -> Command {
    let mut command = match java_home() {
        Some(home) => {
            let mut command = user_environment::command(home.join("bin").join(name));
            command.env("JAVA_HOME", &home);
            command
        }
        None => user_environment::command(name),
    };
    command.env_remove(PASSWORD_VARIABLE);
    command
}

fn apksigner() -> PlayResult<Command> {
    let home = var_os("HOME").map(PathBuf::from);
    let sdks = [
        var_os("ANDROID_HOME").map(PathBuf::from),
        var_os("ANDROID_SDK_ROOT").map(PathBuf::from),
        home.as_ref().map(|home| home.join("Library/Android/sdk")),
        home.as_ref().map(|home| home.join("Android/Sdk")),
    ];
    let newest = sdks
        .into_iter()
        .flatten()
        .filter_map(|sdk| std::fs::read_dir(sdk.join("build-tools")).ok())
        .flat_map(|versions| versions.flatten().map(|version| version.path()))
        .filter(|version| version.join("apksigner").is_file())
        .max_by_key(|version| {
            version
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| {
                    name.split(['.', '-'])
                        .map(|part| part.parse::<u32>().unwrap_or(0))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
        .ok_or_else(|| {
            PlayError::NotFound("no apksigner: install the Android SDK's build-tools".into())
        })?;
    let mut command = user_environment::command(newest.join("apksigner"));
    if let Some(home) = java_home() {
        let path = var_os("PATH").unwrap_or_default();
        let mut paths = vec![home.join("bin")];
        paths.extend(std::env::split_paths(&path));
        command
            .env("JAVA_HOME", &home)
            .env("PATH", std::env::join_paths(paths).unwrap_or(path));
    }
    Ok(command)
}

/// Runs `command` with the password in its environment, answering what it printed.
fn run(mut command: Command, password: Option<&str>) -> PlayResult<String> {
    if let Some(password) = password {
        command.env(PASSWORD_VARIABLE, password);
    }
    let out = command.output().map_err(|error| {
        PlayError::NotFound(format!(
            "{error}: install a JDK or Android Studio for keytool and jarsigner"
        ))
    })?;
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    match out.status.success() {
        true => Ok(said),
        false => Err(PlayError::Api(failure(&said))),
    }
}

/// The line that says what went wrong, rather than the Java stack trace under it.
fn failure(said: &str) -> String {
    let lines = || said.lines().map(str::trim).filter(|line| !line.is_empty());
    let line = lines()
        .find(|line| line.to_ascii_lowercase().contains("error"))
        .or_else(|| lines().next())
        .unwrap_or("failed");
    match line.contains("password was incorrect") {
        true => "that password does not open the keystore".into(),
        false => line.into(),
    }
}

fn line_after<'a>(said: &'a str, label: &str) -> Option<&'a str> {
    said.lines()
        .find_map(|line| line.trim().strip_prefix(label))
        .map(str::trim)
}

/// A certificate digest as Play Console shows it: upper-case pairs of hex digits between colons.
fn fingerprint(raw: &str) -> String {
    let digits: Vec<char> = raw
        .chars()
        .filter(char::is_ascii_hexdigit)
        .map(|digit| digit.to_ascii_uppercase())
        .collect();
    digits
        .chunks(2)
        .map(|pair| pair.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join(":")
}

fn inspect(path: &str, alias: &str, password: &str) -> PlayResult<Value> {
    let mut command = java_tool("keytool");
    command.args([
        "-list",
        "-v",
        "-keystore",
        path,
        "-alias",
        alias,
        "-storepass:env",
    ]);
    command.arg(PASSWORD_VARIABLE);
    let said = run(command, Some(password))?;
    let valid = line_after(&said, "Valid from:").unwrap_or_default();
    Ok(json!({
        "path": path,
        "alias": alias,
        "owner": line_after(&said, "Owner:"),
        "validUntil": valid.split_once("until:").map(|(_, until)| until.trim()),
        "sha1": line_after(&said, "SHA1:").map(fingerprint),
        "sha256": line_after(&said, "SHA256:").map(fingerprint),
        "algorithm": line_after(&said, "Subject Public Key Algorithm:"),
    }))
}

fn password_of(path: &str) -> PlayResult<String> {
    KEYCHAIN.read(&keystore_entry(path))?.ok_or_else(|| {
        PlayError::NotFound(format!(
            "Sikemux has no password for {path}; keep it with play_keystore_add"
        ))
    })
}

fn kept(config: &PlayConfig, path: Option<&str>) -> PlayResult<Keystore> {
    let path = path.map(|path| expand(path).to_string_lossy().into_owned());
    let mut matching = config
        .keystores
        .iter()
        .filter(|kept| path.as_deref().is_none_or(|path| kept.path == path));
    match (matching.next(), matching.next()) {
        (Some(only), None) => Ok(only.clone()),
        (None, _) => Err(PlayError::NotFound(match path {
            Some(path) => format!("Sikemux keeps no keystore at {path}"),
            None => {
                "Sikemux keeps no upload keystore yet; make one with play_keystore_create".into()
            }
        })),
        (Some(_), Some(_)) => Err(PlayError::BadArg(
            "Sikemux keeps several keystores; name one with keystore".into(),
        )),
    }
}

#[derive(Deserialize)]
pub struct KeystoreAsk {
    #[serde(default)]
    pub path: Option<String>,
}

pub async fn keystores(data_dir: &Path, ask: KeystoreAsk) -> PlayResult<Value> {
    let config = config::load(data_dir)?;
    let chosen: Vec<Keystore> = match ask.path {
        Some(path) => vec![kept(&config, Some(&path))?],
        None => config.keystores,
    };
    store::blocking(move || {
        let read = chosen
            .iter()
            .map(|kept| {
                password_of(&kept.path)
                    .and_then(|password| inspect(&kept.path, &kept.alias, &password))
                    .unwrap_or_else(|error| {
                        json!({ "path": kept.path, "alias": kept.alias, "problem": error.to_string() })
                    })
            })
            .collect::<Vec<_>>();
        Ok(json!({ "keystores": read }))
    })
    .await
}

fn outside_repositories(path: &Path) -> PlayResult<()> {
    match path
        .ancestors()
        .skip(1)
        .find(|dir| dir.join(".git").exists())
    {
        Some(repository) => Err(PlayError::BadArg(format!(
            "{} is inside the repository at {}; keep upload keys outside any repository",
            path.display(),
            repository.display()
        ))),
        None => Ok(()),
    }
}

async fn keep(data_dir: &Path, path: String, alias: String, password: String) -> PlayResult<()> {
    store::update(data_dir, move |config: &mut PlayConfig| {
        KEYCHAIN.write(&keystore_entry(&path), &password)?;
        config.keystores.retain(|kept| kept.path != path);
        config.keystores.push(Keystore { path, alias });
        Ok(())
    })
    .await
}

#[derive(Deserialize)]
pub struct CreateAsk {
    pub path: String,
    #[serde(default)]
    pub alias: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

pub async fn create(data_dir: &Path, ask: CreateAsk) -> PlayResult<Value> {
    let path = expand(&ask.path);
    if !path.is_absolute() || path.exists() {
        return Err(PlayError::BadArg(format!(
            "{} is taken or not absolute; name a new file",
            path.display()
        )));
    }
    outside_repositories(&path)?;
    let alias = ask.alias.unwrap_or_else(|| "upload".into());
    let owner = format!("CN={}", ask.name.unwrap_or_else(|| "Upload key".into()));
    let mut random = [0u8; 24];
    SystemRandom::new()
        .fill(&mut random)
        .map_err(|_| PlayError::Transport("no randomness for a password".into()))?;
    let password = URL_SAFE_NO_PAD.encode(random);
    let shown = path.to_string_lossy().into_owned();
    let certificate = path.with_extension("pem");
    let (made, made_alias, made_password) = (shown.clone(), alias.clone(), password.clone());
    let read = store::blocking(move || {
        let mut generate = java_tool("keytool");
        generate.args(["-genkeypair", "-keystore", &made, "-storetype", "PKCS12"]);
        generate.args(["-alias", &made_alias, "-keyalg", "RSA", "-keysize", "4096"]);
        generate.args(["-validity", "10000", "-dname", &owner, "-storepass:env"]);
        generate.arg(PASSWORD_VARIABLE);
        run(generate, Some(&made_password))?;
        let mut export = java_tool("keytool");
        export.args([
            "-exportcert",
            "-rfc",
            "-keystore",
            &made,
            "-alias",
            &made_alias,
        ]);
        export.arg("-file").arg(&certificate);
        export.args(["-storepass:env", PASSWORD_VARIABLE]);
        run(export, Some(&made_password))?;
        inspect(&made, &made_alias, &made_password)
    })
    .await?;
    keep(data_dir, shown, alias, password).await?;
    Ok(read)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddAsk {
    pub path: String,
    pub alias: String,
    pub password_file: String,
}

pub async fn add(data_dir: &Path, ask: AddAsk) -> PlayResult<Value> {
    let path = expand(&ask.path).to_string_lossy().into_owned();
    let secret = expand(&ask.password_file);
    let small = std::fs::metadata(&secret).is_ok_and(|about| about.len() <= MAX_PASSWORD_FILE);
    if !small {
        return Err(PlayError::BadArg(format!(
            "no password file at {}",
            secret.display()
        )));
    }
    let password = tokio::fs::read_to_string(&secret)
        .await?
        .trim_end_matches(['\n', '\r'])
        .to_string();
    let (checked, alias, tried) = (path.clone(), ask.alias.clone(), password.clone());
    let read = store::blocking(move || inspect(&checked, &alias, &tried)).await?;
    keep(data_dir, path, ask.alias, password).await?;
    Ok(read)
}

#[derive(Deserialize)]
pub struct SignAsk {
    pub file: String,
    #[serde(default)]
    pub keystore: Option<String>,
}

pub async fn sign(data_dir: &Path, ask: SignAsk) -> PlayResult<Value> {
    let file = expand(&ask.file);
    let bundle = match file.extension().and_then(|end| end.to_str()) {
        Some("aab") => true,
        Some("apk") => false,
        _ => {
            return Err(PlayError::BadArg(
                "play_sign signs an .aab or an .apk".into(),
            ))
        }
    };
    if !file.is_file() {
        return Err(PlayError::BadArg(format!("no file at {}", file.display())));
    }
    let keystore = kept(&config::load(data_dir)?, ask.keystore.as_deref())?;
    store::blocking(move || {
        let password = password_of(&keystore.path)?;
        let signer = match bundle {
            true => {
                let mut sign = java_tool("jarsigner");
                sign.args([
                    "-keystore",
                    &keystore.path,
                    "-storepass:env",
                    PASSWORD_VARIABLE,
                ]);
                sign.args(["-sigalg", "SHA256withRSA", "-digestalg", "SHA-256"]);
                sign.arg(&file).arg(&keystore.alias);
                run(sign, Some(&password))?;
                let mut verify = java_tool("jarsigner");
                verify.arg("-verify").arg(&file);
                run(verify, None)?;
                let mut print = java_tool("keytool");
                print.arg("-printcert").arg("-jarfile").arg(&file);
                line_after(&run(print, None)?, "SHA256:").map(fingerprint)
            }
            false => {
                let mut sign = apksigner()?;
                sign.args([
                    "sign",
                    "--ks",
                    &keystore.path,
                    "--ks-key-alias",
                    &keystore.alias,
                ]);
                sign.arg("--ks-pass")
                    .arg(format!("env:{PASSWORD_VARIABLE}"));
                sign.arg(&file);
                run(sign, Some(&password))?;
                let mut verify = apksigner()?;
                verify.args(["verify", "--print-certs"]).arg(&file);
                run(verify, None)?
                    .lines()
                    .find_map(|line| line.split_once("certificate SHA-256 digest:"))
                    .map(|(_, digest)| fingerprint(digest))
            }
        };
        Ok(json!({
            "file": file,
            "keystore": keystore.path,
            "alias": keystore.alias,
            "signerSha256": signer,
        }))
    })
    .await
}
