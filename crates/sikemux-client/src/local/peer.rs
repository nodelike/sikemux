//! The process on the other end of a connection the core took, as the OS names it.

#[cfg(target_vendor = "apple")]
#[path = "peer/apple.rs"]
mod platform;
#[cfg(any(target_os = "linux", target_os = "android"))]
#[path = "peer/linux.rs"]
mod platform;
#[cfg(all(
    unix,
    not(any(target_vendor = "apple", target_os = "linux", target_os = "android"))
))]
#[path = "peer/other.rs"]
mod platform;
#[cfg(windows)]
#[path = "peer/windows.rs"]
mod platform;

pub use platform::Peer;

pub fn peer(connection: &super::Connection) -> Option<Peer> {
    platform::peer(connection)
}
