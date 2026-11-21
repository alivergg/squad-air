use if_addrs::{get_if_addrs, IfAddr};
use squad_proto::MAX_LAN_ADDRS;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::{OnceLock, RwLock};

static PLATFORM_LAN_IPV4_OVERRIDE: OnceLock<RwLock<Vec<Ipv4Addr>>> = OnceLock::new();

fn override_store() -> &'static RwLock<Vec<Ipv4Addr>> {
    PLATFORM_LAN_IPV4_OVERRIDE.get_or_init(|| RwLock::new(Vec::new()))
}

pub fn set_platform_lan_ipv4_override(addrs: Vec<Ipv4Addr>) {
    let filtered: Vec<Ipv4Addr> = addrs
        .into_iter()
        .filter(|ip| !ip.is_loopback() && !ip.is_unspecified())
        .take(MAX_LAN_ADDRS)
        .collect();
    if let Ok(mut guard) = override_store().write() {
        *guard = filtered;
    }
}

fn read_platform_lan_ipv4_override() -> Vec<Ipv4Addr> {
    override_store()
        .read()
        .map(|g| g.clone())
        .unwrap_or_default()
}

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
    if out.is_empty() {
        for ip in read_platform_lan_ipv4_override() {
            if !ip.is_loopback() && !ip.is_unspecified() {
                out.push(SocketAddrV4::new(ip, bound_port));
                if out.len() >= MAX_LAN_ADDRS {
                    break;
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
    if addrs.is_empty() {
        for ip in read_platform_lan_ipv4_override() {
            if !ip.is_loopback() && !ip.is_unspecified() {
                let bcast = Ipv4Addr::from(u32::from(ip) | 0x0000_00FF);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_lan_override_filters_invalid_addrs() {
        set_platform_lan_ipv4_override(vec![
            Ipv4Addr::LOCALHOST,
            Ipv4Addr::UNSPECIFIED,
            Ipv4Addr::new(192, 168, 1, 42),
        ]);
        let stored = read_platform_lan_ipv4_override();
        assert_eq!(stored, vec![Ipv4Addr::new(192, 168, 1, 42)]);
        set_platform_lan_ipv4_override(Vec::new());
    }
}
