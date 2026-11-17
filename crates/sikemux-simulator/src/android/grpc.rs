//! The emulator's own gRPC service, which streams its screen to the desk without
//! costing the device anything, as Android Studio's Running Devices window reads it.
//! Only `streamScreenshot` is used, so its two messages are written out by hand.

use bytes::{Buf, BytesMut};
use http::Request;

const STREAM_SCREENSHOT: &str = "/android.emulation.control.EmulatorController/streamScreenshot";
const RGB888: u64 = 2;
/// One frame of a large phone screen fits several times over.
const WINDOW: u32 = 32 << 20;

/// One picture of the screen, upright as the device shows it now: three bytes a pixel.
pub(crate) struct Image {
    pub width: u32,
    pub height: u32,
    pub pixels: bytes::Bytes,
}

/// Streams the screen, scaled to fit `width` by `height` pixels, to `on_image`
/// until it answers false or the emulator stops.
pub(crate) async fn stream_screenshot(
    port: u16,
    token: &str,
    width: u32,
    height: u32,
    mut on_image: impl FnMut(Image) -> bool,
) -> Result<(), String> {
    let socket = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .map_err(|error| format!("could not reach the emulator: {error}"))?;
    socket
        .set_nodelay(true)
        .map_err(|error| error.to_string())?;
    let (client, connection) = h2::client::Builder::new()
        .initial_window_size(WINDOW)
        .initial_connection_window_size(WINDOW)
        .max_frame_size(1 << 20)
        .handshake::<_, bytes::Bytes>(socket)
        .await
        .map_err(|error| format!("the emulator's stream did not open: {error}"))?;
    let connection = tokio::spawn(connection);
    let result = read(client, port, token, width, height, &mut on_image).await;
    connection.abort();
    result
}

async fn read(
    client: h2::client::SendRequest<bytes::Bytes>,
    port: u16,
    token: &str,
    width: u32,
    height: u32,
    on_image: &mut impl FnMut(Image) -> bool,
) -> Result<(), String> {
    let mut client = client.ready().await.map_err(|error| error.to_string())?;
    let request = Request::post(format!("http://127.0.0.1:{port}{STREAM_SCREENSHOT}"))
        .header("content-type", "application/grpc")
        .header("te", "trailers")
        .header("authorization", format!("Bearer {token}"))
        .body(())
        .map_err(|error| error.to_string())?;
    let (response, mut send) = client
        .send_request(request, false)
        .map_err(|error| error.to_string())?;
    let mut format = Vec::new();
    field_varint(&mut format, 1, RGB888);
    field_varint(&mut format, 3, u64::from(width));
    field_varint(&mut format, 4, u64::from(height));
    send.send_data(framed(&format), true)
        .map_err(|error| error.to_string())?;
    let response = response.await.map_err(|error| error.to_string())?;
    refused(response.headers())?;
    let mut body = response.into_body();
    let mut buffer = BytesMut::new();
    while let Some(chunk) = body.data().await {
        let chunk = chunk.map_err(|error| error.to_string())?;
        let _ = body.flow_control().release_capacity(chunk.len());
        buffer.extend_from_slice(&chunk);
        while let Some(message) = message(&mut buffer) {
            if let Some(image) = image(message) {
                if image.width > 0 && !on_image(image) {
                    return Ok(());
                }
            }
        }
    }
    if let Ok(Some(trailers)) = body.trailers().await {
        refused(&trailers)?;
    }
    Err("the emulator stopped streaming its screen".into())
}

fn refused(headers: &http::HeaderMap) -> Result<(), String> {
    let status = headers
        .get("grpc-status")
        .and_then(|status| status.to_str().ok());
    match status {
        None | Some("0") => Ok(()),
        Some(code) => Err(format!(
            "the emulator refused its screen ({code}): {}",
            headers
                .get("grpc-message")
                .and_then(|message| message.to_str().ok())
                .unwrap_or_default()
        )),
    }
}

fn framed(message: &[u8]) -> bytes::Bytes {
    let mut out = Vec::with_capacity(message.len() + 5);
    out.push(0);
    out.extend_from_slice(&(message.len() as u32).to_be_bytes());
    out.extend_from_slice(message);
    out.into()
}

/// The next whole message in `buffer`, once it has all arrived.
fn message(buffer: &mut BytesMut) -> Option<bytes::Bytes> {
    if buffer.len() < 5 {
        return None;
    }
    let length = u32::from_be_bytes([buffer[1], buffer[2], buffer[3], buffer[4]]) as usize;
    if buffer.len() < 5 + length {
        return None;
    }
    buffer.advance(5);
    Some(buffer.split_to(length).freeze())
}

fn field_varint(out: &mut Vec<u8>, field: u64, value: u64) {
    varint(out, field << 3);
    varint(out, value);
}

fn varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn read_varint(data: &mut bytes::Bytes) -> Option<u64> {
    let mut value = 0;
    for shift in (0..64).step_by(7) {
        if !data.has_remaining() {
            return None;
        }
        let byte = data.get_u8();
        value |= u64::from(byte & 0x7f) << shift;
        if byte < 0x80 {
            return Some(value);
        }
    }
    None
}

/// Each field of a message: its number, and its value as a number or as bytes.
fn fields(mut data: bytes::Bytes) -> Vec<(u64, Result<u64, bytes::Bytes>)> {
    let mut found = Vec::new();
    while data.has_remaining() {
        let Some(key) = read_varint(&mut data) else {
            break;
        };
        let value = match key & 7 {
            0 => read_varint(&mut data).map(Ok),
            1 if data.remaining() >= 8 => Some(Ok(data.get_u64_le())),
            2 => read_varint(&mut data)
                .map(|length| length as usize)
                .filter(|length| *length <= data.remaining())
                .map(|length| Err(data.split_to(length))),
            5 if data.remaining() >= 4 => Some(Ok(u64::from(data.get_u32_le()))),
            _ => None,
        };
        let Some(value) = value else {
            break;
        };
        found.push((key >> 3, value));
    }
    found
}

fn image(message: bytes::Bytes) -> Option<Image> {
    let (mut width, mut height, mut pixels) = (0, 0, None);
    for (field, value) in fields(message) {
        match (field, value) {
            (1, Err(format)) => {
                for (field, value) in fields(format) {
                    match (field, value) {
                        (3, Ok(value)) => width = value as u32,
                        (4, Ok(value)) => height = value as u32,
                        _ => {}
                    }
                }
            }
            (4, Err(bytes)) => pixels = Some(bytes),
            _ => {}
        }
    }
    let pixels = pixels.unwrap_or_default();
    (pixels.len() >= (width * height * 3) as usize).then_some(Image {
        width,
        height,
        pixels,
    })
}
