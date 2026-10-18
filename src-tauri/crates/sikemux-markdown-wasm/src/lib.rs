//! `sikemux-markdown` for a page in a browser, which reads chats as the Mac
//! does. The page puts a request's JSON in memory it took with `take`, calls
//! `parse`, and reads the answer's JSON from where `parse` says it is, then
//! gives both back with `give`.

use serde::Deserialize;
use sikemux_markdown::{parse as read, Options};

#[derive(Deserialize)]
struct Request {
    text: String,
    options: Options,
    skip: usize,
}

/// `len` bytes of memory for the page to write into.
#[no_mangle]
pub extern "C" fn take(len: usize) -> *mut u8 {
    leak(vec![0; len].into_boxed_slice())
}

fn leak(bytes: Box<[u8]>) -> *mut u8 {
    Box::into_raw(bytes).cast::<u8>()
}

/// Gives back memory from `take`, or the answer of `parse`.
///
/// # Safety
///
/// `at` and `len` must be what `take` returned and was asked for, or the
/// place and length `parse` answered with, and given back once.
#[no_mangle]
pub unsafe extern "C" fn give(at: *mut u8, len: usize) {
    // SAFETY: the caller hands back a boxed slice of exactly `len` bytes that
    // this module leaked and has not freed.
    drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(at, len)) });
}

/// Reads the request at `at` and answers with where its blocks' JSON is: the
/// place in the high 32 bits and the length in the low. A request that is
/// not one answers with no blocks.
///
/// # Safety
///
/// `at` and `len` must be a buffer from `take` that the page filled.
#[no_mangle]
pub unsafe extern "C" fn parse(at: *const u8, len: usize) -> u64 {
    // SAFETY: the caller filled `len` bytes at `at`, which `take` made.
    let input = unsafe { std::slice::from_raw_parts(at, len) };
    let blocks = serde_json::from_slice::<Request>(input)
        .map(|request| read(&request.text, request.options, request.skip))
        .unwrap_or_default();
    let out = serde_json::to_vec(&blocks)
        .unwrap_or_else(|_| b"[]".to_vec())
        .into_boxed_slice();
    let length = out.len() as u64;
    let place = leak(out) as u64;
    (place << 32) | length
}
