use std::collections::HashSet;

use sikemux_agents::tcp_table;

use super::Listener;

pub fn listeners(pids: &[u32]) -> Vec<Listener> {
    let wanted: HashSet<u32> = pids.iter().copied().collect();
    tcp_table::listening()
        .into_iter()
        .filter(|socket| wanted.contains(&socket.pid))
        .map(|socket| Listener {
            pid: socket.pid,
            port: socket.port,
            address: socket.address,
        })
        .collect()
}
