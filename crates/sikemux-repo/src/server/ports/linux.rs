use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use super::Listener;

const TCP_LISTEN: &str = "0A";

/// `/proc/net/tcp` writes an address as 32-bit words in host order.
fn parse_address(hex: &str) -> Option<IpAddr> {
    let words: Vec<u32> = (0..hex.len() / 8)
        .map(|word| u32::from_str_radix(hex.get(word * 8..word * 8 + 8)?, 16).ok())
        .collect::<Option<_>>()?;
    let bytes: Vec<u8> = words.iter().flat_map(|word| word.to_ne_bytes()).collect();
    match bytes.len() {
        4 => Some(IpAddr::V4(Ipv4Addr::from(<[u8; 4]>::try_from(bytes).ok()?))),
        16 => Some(IpAddr::V6(Ipv6Addr::from(
            <[u8; 16]>::try_from(bytes).ok()?,
        ))),
        _ => None,
    }
}

/// Listening sockets by inode, from a `/proc/net/tcp` or `tcp6` table.
fn parse_table(table: &str, into: &mut HashMap<u64, (u16, IpAddr)>) {
    for line in table.lines().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let (Some(local), Some(state), Some(inode)) = (fields.get(1), fields.get(3), fields.get(9))
        else {
            continue;
        };
        if *state != TCP_LISTEN {
            continue;
        }
        let Some((address, port)) = local.split_once(':') else {
            continue;
        };
        let (Some(address), Ok(port), Ok(inode)) = (
            parse_address(address),
            u16::from_str_radix(port, 16),
            inode.parse(),
        ) else {
            continue;
        };
        if port != 0 && inode != 0 {
            into.insert(inode, (port, address));
        }
    }
}

fn socket_inodes(pid: u32) -> Vec<u64> {
    let Ok(entries) = std::fs::read_dir(format!("/proc/{pid}/fd")) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let target = std::fs::read_link(entry.path()).ok()?;
            target
                .to_str()?
                .strip_prefix("socket:[")?
                .strip_suffix(']')?
                .parse()
                .ok()
        })
        .collect()
}

pub fn listeners(pids: &[u32]) -> Vec<Listener> {
    let mut listening = HashMap::new();
    for table in ["/proc/net/tcp", "/proc/net/tcp6"] {
        if let Ok(text) = std::fs::read_to_string(table) {
            parse_table(&text, &mut listening);
        }
    }
    if listening.is_empty() {
        return Vec::new();
    }
    pids.iter()
        .flat_map(|&pid| {
            socket_inodes(pid)
                .into_iter()
                .filter_map(|inode| listening.get(&inode).copied())
                .map(move |(port, address)| Listener { pid, port, address })
                .collect::<Vec<_>>()
        })
        .collect()
}
