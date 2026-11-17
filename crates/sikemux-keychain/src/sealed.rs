//! Secrets in the files `file` keeps, each sealed with Windows' data
//! protection for the signed-in person, so only a program running as them on
//! this computer can open it, and a copied file opens nowhere else.

use std::path::Path;
use std::ptr;

use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
};

use crate::{file, KeychainError, KeychainResult};

pub(crate) fn read(root: &Path, service: &str, account: &str) -> KeychainResult<Option<String>> {
    let Some(sealed) = file::read_bytes(root, service, account)? else {
        return Ok(None);
    };
    let secret =
        String::from_utf8(unseal(&sealed)?).map_err(|_| failed("a saved secret is not text"))?;
    Ok(Some(secret.trim().to_owned()).filter(|secret| !secret.is_empty()))
}

pub(crate) fn write(root: &Path, service: &str, account: &str, secret: &str) -> KeychainResult<()> {
    file::write_bytes(root, service, account, &seal(secret.as_bytes())?)
}

fn failed(why: impl std::fmt::Display) -> KeychainError {
    KeychainError::Failed(format!("Windows would not seal or open the secret: {why}"))
}

fn blob(bytes: &[u8]) -> KeychainResult<CRYPT_INTEGER_BLOB> {
    Ok(CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(bytes.len()).map_err(|_| failed("it is too long"))?,
        pbData: bytes.as_ptr().cast_mut(),
    })
}

/// Copies out what Windows allocated, then frees it.
fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
    if out.pbData.is_null() {
        return Vec::new();
    }
    // SAFETY: a call that succeeded left `cbData` bytes at `pbData`, allocated with LocalAlloc.
    let bytes = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec();
    // SAFETY: `pbData` came from LocalAlloc and nothing else holds it.
    unsafe { LocalFree(out.pbData.cast()) };
    bytes
}

fn seal(secret: &[u8]) -> KeychainResult<Vec<u8>> {
    let input = blob(secret)?;
    let mut out = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: ptr::null_mut(),
    };
    // SAFETY: `input` points into `secret`, which outlives the call, and Windows only reads it;
    // `out` is ours to fill.
    let sealed = unsafe {
        CryptProtectData(
            &input,
            ptr::null(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )
    };
    if sealed == 0 {
        return Err(failed(std::io::Error::last_os_error()));
    }
    Ok(take(out))
}

fn unseal(sealed: &[u8]) -> KeychainResult<Vec<u8>> {
    let input = blob(sealed)?;
    let mut out = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: ptr::null_mut(),
    };
    // SAFETY: as in `seal`; Windows writes no description, since none is asked for.
    let opened = unsafe {
        CryptUnprotectData(
            &input,
            ptr::null_mut(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )
    };
    if opened == 0 {
        return Err(failed(std::io::Error::last_os_error()));
    }
    Ok(take(out))
}
