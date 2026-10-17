use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddrV4, UdpSocket};

use baton_core::config::WakeMethod;
use baton_core::ids::MacAddr;
use baton_core::ports::{WakeOutcome, WakeTarget, Waker};
use baton_core::{Error, Result};

pub const WOL_PORT: u16 = 9;

/// 6 字节 `0xFF` 后接 16 次目标 MAC 地址。
pub fn magic_packet(mac: MacAddr) -> [u8; 102] {
    let mut packet = [0xFF; 102];
    for chunk in packet[6..].as_chunks_mut::<6>().0 {
        *chunk = mac.0;
    }
    packet
}

/// 魔术包的目的地址：受限广播、本机各网卡的子网广播、对端最后已知的单播 IP，去重。
pub fn destinations(subnet_broadcasts: &[Ipv4Addr], last_ip: Option<IpAddr>) -> Vec<SocketAddrV4> {
    let unicast = match last_ip {
        Some(IpAddr::V4(ip)) => Some(ip),
        Some(IpAddr::V6(ip)) => ip.to_ipv4_mapped(),
        None => None,
    };
    let mut out: Vec<SocketAddrV4> = Vec::new();
    for ip in std::iter::once(Ipv4Addr::BROADCAST)
        .chain(subnet_broadcasts.iter().copied())
        .chain(unicast)
    {
        let addr = SocketAddrV4::new(ip, WOL_PORT);
        if !out.contains(&addr) {
            out.push(addr);
        }
    }
    out
}

fn subnet_broadcasts() -> Vec<Ipv4Addr> {
    netdev::get_interfaces()
        .iter()
        .flat_map(|i| &i.ipv4)
        .filter(|n| !n.addr().is_loopback() && n.prefix_len() < 31)
        .map(|n| n.broadcast())
        .collect()
}

/// Wake-on-LAN。有线的 Windows 与 Wi-Fi 下合盖睡眠的 Mac 都适用（后者需目标 MAC 为 `en0` 当前在用的地址）。
#[derive(Debug, Default)]
pub struct WolWaker;

impl WolWaker {
    /// 发往全部目的地址，任一成功即算发出；返回成功发出的个数。
    pub fn send(&self, mac: MacAddr, last_ip: Option<IpAddr>) -> io::Result<usize> {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
        socket.set_broadcast(true)?;
        let packet = magic_packet(mac);
        let mut sent = 0;
        let mut last_error = None;
        for dest in destinations(&subnet_broadcasts(), last_ip) {
            match socket.send_to(&packet, dest) {
                Ok(_) => sent += 1,
                Err(e) => {
                    log::debug!("向 {dest} 发送魔术包失败：{e}");
                    last_error = Some(e);
                }
            }
        }
        match (sent, last_error) {
            (0, Some(e)) => Err(e),
            _ => Ok(sent),
        }
    }
}

impl Waker for WolWaker {
    fn method(&self) -> WakeMethod {
        WakeMethod::Wol
    }

    fn wake(&self, target: &WakeTarget) -> Result<WakeOutcome> {
        let mac = target.wol_mac.ok_or_else(|| {
            Error::Wake(format!(
                "不知道 {} 的 MAC 地址：先让它上线一次，或在配置中填写 wol_mac",
                target.device_id.0
            ))
        })?;
        self.send(mac, target.last_ip)
            .map_err(|e| Error::Wake(e.to_string()))?;
        Ok(WakeOutcome::Sent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_packet_layout() {
        let mac = MacAddr([0x01, 0x23, 0x45, 0x67, 0x89, 0xAB]);
        let packet = magic_packet(mac);
        assert_eq!(&packet[..6], &[0xFF; 6]);
        assert!(packet[6..].as_chunks::<6>().0.iter().all(|c| *c == mac.0));
    }

    #[test]
    fn destinations_cover_broadcasts_and_unicast_once() {
        let subnet = Ipv4Addr::new(192, 168, 1, 255);
        let dests = destinations(
            &[subnet, subnet, Ipv4Addr::BROADCAST],
            Some("192.168.1.20".parse().unwrap()),
        );
        assert_eq!(
            dests,
            [
                SocketAddrV4::new(Ipv4Addr::BROADCAST, WOL_PORT),
                SocketAddrV4::new(subnet, WOL_PORT),
                SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 20), WOL_PORT),
            ]
        );
    }

    #[test]
    fn destinations_without_known_ip() {
        assert_eq!(
            destinations(&[], None),
            [SocketAddrV4::new(Ipv4Addr::BROADCAST, WOL_PORT)]
        );
        assert_eq!(
            destinations(&[], Some("fe80::1".parse().unwrap())).len(),
            1,
            "非 IPv4 映射的 IPv6 地址不作单播目标"
        );
    }
}
