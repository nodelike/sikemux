use std::ffi::c_void;
use std::net::IpAddr;

const PROC_PIDFDSOCKETINFO: libc::c_int = 3;

fn socket_fds(pid: u32) -> Vec<i32> {
    let entry = size_of::<libc::proc_fdinfo>();
    // SAFETY: a null buffer asks only for the size the fd list needs.
    let needed = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDLISTFDS,
            0,
            std::ptr::null_mut(),
            0,
        )
    };
    if needed <= 0 {
        return Vec::new();
    }
    let capacity = needed as usize / entry + 16;
    let mut fds: Vec<libc::proc_fdinfo> = Vec::with_capacity(capacity);
    // SAFETY: the buffer has room for `capacity` entries and its byte length is passed.
    let written = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDLISTFDS,
            0,
            fds.as_mut_ptr().cast::<c_void>(),
            (capacity * entry) as libc::c_int,
        )
    };
    if written <= 0 {
        return Vec::new();
    }
    // SAFETY: the kernel initialised `written` bytes of whole entries.
    unsafe { fds.set_len((written as usize / entry).min(capacity)) };
    fds.into_iter()
        .filter(|fd| fd.proc_fdtype == libc::PROX_FDTYPE_SOCKET as u32)
        .map(|fd| fd.proc_fd)
        .collect()
}

fn listeners_of(pid: u32) -> Vec<(u16, IpAddr)> {
    socket_fds(pid)
        .into_iter()
        .filter_map(|fd| {
            let mut buffer = [0u64; socket_fdinfo::SIZE / 8];
            // SAFETY: the buffer is exactly `socket_fdinfo::SIZE` bytes.
            let written = unsafe {
                libc::proc_pidfdinfo(
                    pid as i32,
                    fd,
                    PROC_PIDFDSOCKETINFO,
                    buffer.as_mut_ptr().cast::<c_void>(),
                    socket_fdinfo::SIZE as libc::c_int,
                )
            };
            if written as usize != socket_fdinfo::SIZE {
                return None;
            }
            // SAFETY: a u64 array reinterpreted as its own bytes.
            let bytes = unsafe {
                std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), socket_fdinfo::SIZE)
            };
            parse_tcp_listener(bytes)
        })
        .collect()
}

pub fn listeners(pids: &[u32]) -> Vec<super::Listener> {
    pids.iter()
        .flat_map(|&pid| {
            listeners_of(pid)
                .into_iter()
                .map(move |(port, address)| super::Listener { pid, port, address })
        })
        .collect()
}

/// Byte offsets into `struct socket_fdinfo` from <sys/proc_info.h>, which libc
/// does not bind.
mod socket_fdinfo {
    pub const SIZE: usize = 792;
    pub const KIND: usize = 256;
    pub const LOCAL_PORT: usize = 268;
    pub const IP_VERSION: usize = 288;
    pub const LOCAL_ADDRESS: usize = 312;
    pub const TCP_STATE: usize = 344;
    pub const KIND_TCP: i32 = 2;
    pub const STATE_LISTEN: i32 = 1;
    pub const IPV4: u8 = 1;
    pub const IPV6: u8 = 2;
}

fn read_i32(bytes: &[u8], at: usize) -> Option<i32> {
    Some(i32::from_ne_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn parse_tcp_listener(bytes: &[u8]) -> Option<(u16, IpAddr)> {
    use socket_fdinfo::*;
    if bytes.len() < SIZE
        || read_i32(bytes, KIND)? != KIND_TCP
        || read_i32(bytes, TCP_STATE)? != STATE_LISTEN
    {
        return None;
    }
    let port = u16::from_be(read_i32(bytes, LOCAL_PORT)? as u16);
    let address = match *bytes.get(IP_VERSION)? {
        flags if flags & IPV6 != 0 => {
            let octets: [u8; 16] = bytes
                .get(LOCAL_ADDRESS..LOCAL_ADDRESS + 16)?
                .try_into()
                .ok()?;
            IpAddr::from(octets)
        }
        flags if flags & IPV4 != 0 => {
            let octets: [u8; 4] = bytes
                .get(LOCAL_ADDRESS + 12..LOCAL_ADDRESS + 16)?
                .try_into()
                .ok()?;
            IpAddr::from(octets)
        }
        _ => return None,
    };
    (port != 0).then_some((port, address))
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    use super::*;

    const ANY6: IpAddr = IpAddr::V6(Ipv6Addr::UNSPECIFIED);
    const LOOP4: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

    fn fdinfo(kind: i32, state: i32, version: u8, port: u16, address: &[u8]) -> Vec<u8> {
        use socket_fdinfo::*;
        let mut bytes = vec![0u8; SIZE];
        bytes[KIND..KIND + 4].copy_from_slice(&kind.to_ne_bytes());
        bytes[TCP_STATE..TCP_STATE + 4].copy_from_slice(&state.to_ne_bytes());
        bytes[IP_VERSION] = version;
        let raw = i32::from(u16::from_ne_bytes(port.to_be_bytes()));
        bytes[LOCAL_PORT..LOCAL_PORT + 4].copy_from_slice(&raw.to_ne_bytes());
        let at = if address.len() == 4 {
            LOCAL_ADDRESS + 12
        } else {
            LOCAL_ADDRESS
        };
        bytes[at..at + address.len()].copy_from_slice(address);
        bytes
    }

    #[test]
    fn socket_info_parses_listening_tcp_only() {
        use socket_fdinfo::*;
        assert_eq!(
            parse_tcp_listener(&fdinfo(KIND_TCP, STATE_LISTEN, IPV4, 5173, &[127, 0, 0, 1])),
            Some((5173, LOOP4))
        );
        assert_eq!(
            parse_tcp_listener(&fdinfo(KIND_TCP, STATE_LISTEN, IPV6, 8080, &[0; 16])),
            Some((8080, ANY6))
        );
        assert_eq!(
            parse_tcp_listener(&fdinfo(KIND_TCP, 4, IPV4, 5173, &[127, 0, 0, 1])),
            None
        );
        assert_eq!(
            parse_tcp_listener(&fdinfo(1, STATE_LISTEN, IPV4, 5173, &[127, 0, 0, 1])),
            None
        );
        assert_eq!(parse_tcp_listener(&[0u8; 12]), None);
    }

    #[test]
    fn the_kernel_reports_a_socket_this_process_listens_on() {
        let socket = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = socket.local_addr().expect("address").port();
        let found = listeners_of(std::process::id());
        assert!(found.contains(&(port, LOOP4)), "{found:?} lacks {port}");
    }
}
