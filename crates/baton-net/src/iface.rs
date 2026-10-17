//! 本机网卡信息：按连接的本地 IP 找到承载它的网卡，取其当前 MAC 作为 WoL 目标上报给对端。
//!
//! Mac 启用私有 Wi-Fi 地址时，网卡当前的 MAC 就是私有地址，正是魔术包要发往的地址。

use std::net::IpAddr;

use baton_core::ids::MacAddr;

pub fn mac_for_local_ip(ip: IpAddr) -> Option<MacAddr> {
    if ip.is_loopback() || ip.is_unspecified() {
        return None;
    }
    let interfaces = netdev::get_interfaces();
    let found = interfaces.iter().find(|i| {
        i.ipv4.iter().any(|n| IpAddr::V4(n.addr()) == ip)
            || i.ipv6.iter().any(|n| IpAddr::V6(n.addr()) == ip)
    })?;
    found
        .mac_addr
        .map(|m| MacAddr(m.octets()))
        .filter(|m| m.0 != [0; 6])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_has_no_mac() {
        assert_eq!(mac_for_local_ip("127.0.0.1".parse().unwrap()), None);
        assert_eq!(mac_for_local_ip("0.0.0.0".parse().unwrap()), None);
    }
}
