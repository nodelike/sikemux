use std::collections::HashSet;
use std::mem::size_of;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, NO_ERROR};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCPROW_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER,
};
use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};

use super::Listener;

fn listening_table(family: u16) -> Vec<u32> {
    let mut size = 0u32;
    let mut table: Vec<u32> = Vec::new();
    for _ in 0..4 {
        // SAFETY: `size` is the byte length of `table`, which the call writes no further than.
        let result = unsafe {
            GetExtendedTcpTable(
                table.as_mut_ptr().cast(),
                &mut size,
                0,
                u32::from(family),
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            )
        };
        match result {
            NO_ERROR => return table,
            ERROR_INSUFFICIENT_BUFFER => table = vec![0; (size as usize).div_ceil(4)],
            _ => return Vec::new(),
        }
    }
    Vec::new()
}

fn rows<Row: Copy>(table: &[u32]) -> Vec<Row> {
    let Some(&count) = table.first() else {
        return Vec::new();
    };
    let words_per_row = size_of::<Row>() / size_of::<u32>();
    let available = (table.len() - 1) / words_per_row;
    let start = table[1..].as_ptr().cast::<Row>();
    (0..(count as usize).min(available))
        // SAFETY: `index` stays below the rows that fit in `table` after its count.
        .map(|index| unsafe { start.add(index).read_unaligned() })
        .collect()
}

fn port(network_order: u32) -> u16 {
    u16::from_be(network_order as u16)
}

pub fn listeners(pids: &[u32]) -> Vec<Listener> {
    let wanted: HashSet<u32> = pids.iter().copied().collect();
    let v4 = rows::<MIB_TCPROW_OWNER_PID>(&listening_table(AF_INET))
        .into_iter()
        .map(|row| Listener {
            pid: row.dwOwningPid,
            port: port(row.dwLocalPort),
            address: IpAddr::V4(Ipv4Addr::from(row.dwLocalAddr.to_ne_bytes())),
        });
    let v6 = rows::<MIB_TCP6ROW_OWNER_PID>(&listening_table(AF_INET6))
        .into_iter()
        .map(|row| Listener {
            pid: row.dwOwningPid,
            port: port(row.dwLocalPort),
            address: IpAddr::V6(Ipv6Addr::from(row.ucLocalAddr)),
        });
    v4.chain(v6)
        .filter(|listener| wanted.contains(&listener.pid))
        .collect()
}
