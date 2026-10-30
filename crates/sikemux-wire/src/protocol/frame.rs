//! How messages are framed. Every frame is a 4-byte big-endian length, then
//! one kind byte, then the payload. The length counts the kind byte and the
//! payload. Control frames carry JSON; terminal bytes travel raw in their own
//! frame kinds.

use std::io::{self, Read};

use crate::pty::shell_protocol::ShellMetadataSnapshot;
use serde::{Deserialize, Serialize};

use super::{RequestId, SessionId, StreamId, MAX_FRAME_BYTES};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameKind {
    Control = 0,
    /// Core to client, a terminal's cells changing: stream id (u32 BE), then
    /// a frame as [`crate::grid`] lays it out.
    Grid = 1,
    /// Core to client, answering an attach: request id (u64 BE), session id
    /// (u64 BE), stream id (u32 BE), header length (u32 BE), [`AttachHeader`]
    /// JSON, then the first frame of the stream, holding the whole screen.
    Snapshot = 2,
    /// Client to core, a write: request id (u64 BE), session id (u64 BE), then
    /// the bytes for the terminal. Answered like any other request.
    Input = 3,
    /// Core to client, answering [`Request::TerminalLines`]: request id
    /// (u64 BE), then the lines as [`crate::grid`] lays them out.
    Lines = 4,
    /// A request every core answers, whatever protocol version it speaks:
    /// sent instead of a hello, and answered once.
    Frozen = 0x46,
}

impl FrameKind {
    fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Control),
            1 => Some(Self::Grid),
            2 => Some(Self::Snapshot),
            3 => Some(Self::Input),
            4 => Some(Self::Lines),
            0x46 => Some(Self::Frozen),
            _ => None,
        }
    }
}

pub fn encode_frozen<T: Serialize>(message: &T) -> serde_json::Result<Vec<u8>> {
    Ok(encode_frame(
        FrameKind::Frozen,
        &[&serde_json::to_vec(message)?],
    ))
}

#[derive(Debug)]
pub struct Frame {
    pub kind: FrameKind,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachHeader {
    pub shell: Option<ShellMetadataSnapshot>,
    /// The session's `Exited` event was sent before this snapshot, so the
    /// client will not hear of the exit again.
    pub exited: bool,
}

pub fn encode_frame(kind: FrameKind, parts: &[&[u8]]) -> Vec<u8> {
    let payload_len: usize = parts.iter().map(|part| part.len()).sum();
    let mut frame = Vec::with_capacity(5 + payload_len);
    frame.extend_from_slice(&((payload_len + 1) as u32).to_be_bytes());
    frame.push(kind as u8);
    for part in parts {
        frame.extend_from_slice(part);
    }
    frame
}

pub fn encode_control<T: Serialize>(message: &T) -> serde_json::Result<Vec<u8>> {
    Ok(encode_frame(
        FrameKind::Control,
        &[&serde_json::to_vec(message)?],
    ))
}

pub fn encode_grid(stream: StreamId, frame: &[u8]) -> Vec<u8> {
    encode_frame(FrameKind::Grid, &[&stream.to_be_bytes(), frame])
}

pub fn encode_input(request_id: RequestId, id: SessionId, bytes: &[u8]) -> Vec<u8> {
    encode_frame(
        FrameKind::Input,
        &[&request_id.to_be_bytes(), &id.to_be_bytes(), bytes],
    )
}

pub fn encode_snapshot(
    request_id: RequestId,
    id: SessionId,
    stream: StreamId,
    header: &AttachHeader,
    frame: &[u8],
) -> serde_json::Result<Vec<u8>> {
    let header = serde_json::to_vec(header)?;
    Ok(encode_frame(
        FrameKind::Snapshot,
        &[
            &request_id.to_be_bytes(),
            &id.to_be_bytes(),
            &stream.to_be_bytes(),
            &(header.len() as u32).to_be_bytes(),
            &header,
            frame,
        ],
    ))
}

pub fn encode_lines(request_id: RequestId, lines: &[u8]) -> Vec<u8> {
    encode_frame(FrameKind::Lines, &[&request_id.to_be_bytes(), lines])
}

fn split_u64(bytes: &[u8]) -> Option<(u64, &[u8])> {
    let (head, rest) = bytes.split_first_chunk::<8>()?;
    Some((u64::from_be_bytes(*head), rest))
}

fn split_u32(bytes: &[u8]) -> Option<(u32, &[u8])> {
    let (head, rest) = bytes.split_first_chunk::<4>()?;
    Some((u32::from_be_bytes(*head), rest))
}

pub fn decode_grid(payload: &[u8]) -> Option<(StreamId, &[u8])> {
    split_u32(payload)
}

pub fn decode_input(payload: &[u8]) -> Option<(RequestId, SessionId, &[u8])> {
    let (request_id, rest) = split_u64(payload)?;
    let (id, bytes) = split_u64(rest)?;
    Some((request_id, id, bytes))
}

/// The parts of an attach's answer.
pub struct Snapshot<'a> {
    pub request_id: RequestId,
    pub id: SessionId,
    pub stream: StreamId,
    pub header: AttachHeader,
    pub frame: &'a [u8],
}

pub fn decode_snapshot(payload: &[u8]) -> Option<Snapshot<'_>> {
    let (request_id, rest) = split_u64(payload)?;
    let (id, rest) = split_u64(rest)?;
    let (stream, rest) = split_u32(rest)?;
    let (header_len, rest) = split_u32(rest)?;
    let header_len = header_len as usize;
    if header_len > rest.len() {
        return None;
    }
    let (header, frame) = rest.split_at(header_len);
    let header = serde_json::from_slice(header).ok()?;
    Some(Snapshot {
        request_id,
        id,
        stream,
        header,
        frame,
    })
}

pub fn decode_lines(payload: &[u8]) -> Option<(RequestId, &[u8])> {
    split_u64(payload)
}

/// Whether a frame from [`encode_frame`] is small enough for the other side
/// to read.
pub fn fits(frame: &[u8]) -> bool {
    frame.len() <= MAX_FRAME_BYTES + 5
}

fn frame_length(header: [u8; 4], max_payload: usize) -> io::Result<usize> {
    let length = u32::from_be_bytes(header) as usize;
    if length == 0 || length > max_payload + 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame length {length} is out of range"),
        ));
    }
    Ok(length)
}

/// A frame is read in chunks that double from this size, so a peer that
/// declares a long frame without sending it never gets room made for it.
const FIRST_CHUNK_BYTES: usize = 64 * 1024;

/// Grows `body` toward `length` and answers with the new room to read into.
fn next_chunk(body: &mut Vec<u8>, length: usize) -> Option<&mut [u8]> {
    let start = body.len();
    if start >= length {
        return None;
    }
    let end = (start * 2).max(FIRST_CHUNK_BYTES).min(length);
    body.resize(end, 0);
    body.get_mut(start..)
}

fn split_frame(mut body: Vec<u8>) -> io::Result<Frame> {
    let kind = body
        .first()
        .copied()
        .and_then(FrameKind::from_byte)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "unknown frame kind"))?;
    body.remove(0);
    Ok(Frame {
        kind,
        payload: body,
    })
}

pub fn read_frame_sync<R: Read>(reader: &mut R) -> io::Result<Option<Frame>> {
    let mut header = [0u8; 4];
    match reader.read_exact(&mut header) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let length = frame_length(header, MAX_FRAME_BYTES)?;
    let mut body = Vec::new();
    while let Some(chunk) = next_chunk(&mut body, length) {
        reader.read_exact(chunk)?;
    }
    split_frame(body).map(Some)
}

#[cfg(unix)]
pub async fn read_frame<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut R,
) -> io::Result<Option<Frame>> {
    read_frame_within(reader, MAX_FRAME_BYTES).await
}

/// Refuses a frame longer than `max_payload` before reading it.
#[cfg(unix)]
pub async fn read_frame_within<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut R,
    max_payload: usize,
) -> io::Result<Option<Frame>> {
    use tokio::io::AsyncReadExt;
    let mut header = [0u8; 4];
    match reader.read_exact(&mut header).await {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let length = frame_length(header, max_payload)?;
    let mut body = Vec::new();
    while let Some(chunk) = next_chunk(&mut body, length) {
        reader.read_exact(chunk).await?;
    }
    split_frame(body).map(Some)
}
