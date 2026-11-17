use if_addrs::{get_if_addrs, IfAddr};
use squad_proto::MAX_LAN_ADDRS;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

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

pub fn discover_lan_broadcast_addrs(port: u16) -> Vec<SocketAddr> {
    let mut addrs = Vec::new();
    if let Ok(interfaces) = get_if_addrs() {
        for iface in interfaces {
            if iface.is_loopback() {
                continue;
            }
            if let IfAddr::V4(v4) = iface.addr {
                if v4.ip.is_loopback() || v4.ip.is_unspecified() {
                    continue;
                }
                let ip_u32 = u32::from(v4.ip);
                let mask_u32 = u32::from(v4.netmask);
                if mask_u32 == 0 {
                    continue;
                }
                let bcast = Ipv4Addr::from(ip_u32 | !mask_u32);
                let target = SocketAddr::V4(SocketAddrV4::new(bcast, port));
                if !addrs.contains(&target) {
                    addrs.push(target);
                }
            }
        }
    }
    let global = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::BROADCAST, port));
    if !addrs.contains(&global) {
        addrs.push(global);
    }
    addrs
}
