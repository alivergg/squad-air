use if_addrs::{get_if_addrs, IfAddr};
use squad_proto::MAX_LAN_ADDRS;
use std::net::SocketAddrV4;

pub fn discover_lan_ipv4_addrs(bound_port: u16) -> Vec<SocketAddrV4> {
    let mut out = Vec::new();
    if let Ok(interfaces) = get_if_addrs() {
        for iface in interfaces {
            if iface.is_loopback() {
                continue;
            }
            if let IfAddr::V4(v4) = iface.addr {
                let ip = v4.ip;
                if !ip.is_loopback() && !ip.is_unspecified() {
                    out.push(SocketAddrV4::new(ip, bound_port));
                    if out.len() >= MAX_LAN_ADDRS {
                        break;
                    }
                }
            }
        }
    }
    out
}
