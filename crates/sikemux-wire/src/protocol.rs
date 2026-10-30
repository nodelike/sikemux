//! Wire format between the core and its clients.

mod agent;
pub mod ask;
mod chats;
mod device;
mod event;
mod frame;
mod layout;
mod message;
mod prefs;
mod session;
mod workspace;

pub use agent::*;
pub use chats::*;
pub use device::*;
pub use event::*;
pub use frame::*;
pub use layout::*;
pub use message::*;
pub use prefs::*;
pub use session::*;
pub use workspace::*;

pub const PROTOCOL: &str = "sikemux-core";
pub const PROTOCOL_VERSION: u32 = 13;
/// The oldest version a device may speak and still be served. A change a
/// device from an older release can still read bumps only `PROTOCOL_VERSION`,
/// so phones waiting on an app store review keep working.
pub const OLDEST_PROTOCOL_VERSION: u32 = 13;
/// How long a core waits for a sleeping chat it was asked to wake to come
/// back up, its agent's adapter and CLI with it. A device waits longer, so the
/// core's reason for giving up reaches it.
pub const WAKE_WAIT: std::time::Duration = std::time::Duration::from_secs(30);
/// Room for the largest attach snapshot plus its header.
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
/// The largest file a device may send for a chat. Sent as base64, it still
/// fits in one frame.
pub const MAX_ATTACHMENT_BYTES: usize = 10 * 1024 * 1024;

pub type SessionId = u64;
/// One attach's stream of frames. A client can watch a session in several
/// places at once, each with its own stream.
pub type StreamId = u32;
pub type RequestId = u64;
pub type CallId = u64;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_carry_a_big_endian_length_that_counts_the_kind_byte() {
        let frame = encode_grid(7, b"hi");
        assert_eq!(&frame[..4], &7u32.to_be_bytes());
        assert_eq!(frame[4], FrameKind::Grid as u8);
        let parsed = read_frame_sync(&mut &frame[..]).unwrap().unwrap();
        assert_eq!(parsed.kind, FrameKind::Grid);
        assert_eq!(decode_grid(&parsed.payload), Some((7, &b"hi"[..])));
    }

    #[test]
    fn snapshot_frames_round_trip_header_and_frame() {
        let header = AttachHeader {
            shell: None,
            exited: true,
        };
        let frame = encode_snapshot(3, 9, 5, &header, b"\xff\x00frame").unwrap();
        let parsed = read_frame_sync(&mut &frame[..]).unwrap().unwrap();
        let snapshot = decode_snapshot(&parsed.payload).unwrap();
        assert_eq!(
            (snapshot.request_id, snapshot.id, snapshot.stream),
            (3, 9, 5)
        );
        assert_eq!(snapshot.header, header);
        assert_eq!(snapshot.frame, b"\xff\x00frame");
    }

    #[test]
    fn oversized_and_empty_frames_are_rejected() {
        let mut empty = &0u32.to_be_bytes()[..];
        assert!(read_frame_sync(&mut empty).is_err());
        let huge = ((MAX_FRAME_BYTES + 2) as u32).to_be_bytes();
        assert!(read_frame_sync(&mut &huge[..]).is_err());
        assert!(read_frame_sync(&mut &[][..]).unwrap().is_none());
    }

    #[tokio::test]
    async fn a_frame_past_the_reader_s_limit_is_refused_before_it_is_read() {
        let frame = encode_control(&"x".repeat(100)).unwrap();
        assert!(read_frame_within(&mut &frame[..], 64).await.is_err());
        assert!(read_frame_within(&mut &frame[..], 200)
            .await
            .unwrap()
            .is_some());
    }

    #[test]
    fn control_messages_are_tagged_camel_case_json() {
        let message = ClientMessage::Request {
            request_id: 4,
            request: Request::Shutdown(ask::Shutdown { stop_all: true }),
        };
        let value = serde_json::to_value(&message).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "type": "request",
                "requestId": 4,
                "request": { "op": "shutdown", "stopAll": true }
            })
        );
        let event = ServerMessage::Event {
            event: Event::Exited {
                id: 2,
                code: Some(0),
                signal: None,
                killed: false,
            },
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(matches!(
            serde_json::from_str::<ServerMessage>(&json).unwrap(),
            ServerMessage::Event {
                event: Event::Exited { id: 2, .. }
            }
        ));
    }
}
