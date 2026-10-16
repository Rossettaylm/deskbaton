use std::io;
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};

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

/// Wake-on-LAN。只适合有线网络的目标（如 Windows 台式机）；Wi-Fi 下的 Mac 基本无效。
pub struct WolWaker {
    pub broadcast: SocketAddrV4,
}

impl Default for WolWaker {
    fn default() -> Self {
        Self {
            broadcast: SocketAddrV4::new(Ipv4Addr::BROADCAST, WOL_PORT),
        }
    }
}

impl WolWaker {
    pub fn send(&self, mac: MacAddr) -> io::Result<()> {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
        socket.set_broadcast(true)?;
        socket.send_to(&magic_packet(mac), self.broadcast)?;
        Ok(())
    }
}

impl Waker for WolWaker {
    fn method(&self) -> WakeMethod {
        WakeMethod::Wol
    }

    fn wake(&self, target: &WakeTarget) -> Result<WakeOutcome> {
        let mac = target
            .wol_mac
            .ok_or_else(|| Error::Wake(format!("{} 未配置 WoL MAC 地址", target.device_id.0)))?;
        self.send(mac).map_err(|e| Error::Wake(e.to_string()))?;
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
}
