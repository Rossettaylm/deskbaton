//! mDNS 发现：广播本机的 `_deskbaton._tcp.local.` 服务，浏览局域网里的其他设备，按证书指纹归档。

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;

use baton_core::ids::DeviceId;
use baton_core::protocol::PROTOCOL_VERSION;
use baton_core::{Error, Result};
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use serde::Serialize;

use crate::auth::{self, Fingerprint};

pub const SERVICE_TYPE: &str = "_deskbaton._tcp.local.";
pub const DEFAULT_PORT: u16 = 47800;

/// 局域网里发现的一台设备。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Discovered {
    pub name: DeviceId,
    /// 证书指纹，十六进制。
    pub fingerprint: String,
    pub addresses: Vec<SocketAddr>,
    /// 对端广播的协议版本；旧版本没有广播或无法解析时为 `None`。
    #[serde(skip)]
    pub version: Option<u32>,
}

impl Discovered {
    pub fn fingerprint(&self) -> Option<Fingerprint> {
        auth::parse_fingerprint(&self.fingerprint)
    }
}

pub struct Discovery {
    daemon: ServiceDaemon,
    found: Arc<Mutex<HashMap<String, Discovered>>>,
    fingerprint: String,
    port: u16,
    fullname: Mutex<String>,
}

impl Discovery {
    /// 注册本机服务并开始浏览；发现的设备增减时调用 `on_change`。
    pub fn start(
        name: &DeviceId,
        fingerprint: &Fingerprint,
        port: u16,
        on_change: impl Fn() + Send + 'static,
    ) -> Result<Self> {
        let mdns = |e: mdns_sd::Error| Error::Net(format!("mDNS：{e}"));
        let daemon = ServiceDaemon::new().map_err(mdns)?;
        let own = auth::to_hex(fingerprint);
        // 主机名只能是 DNS 标签，设备名可能含空格或中文，所以用指纹前缀。
        let host = format!("deskbaton-{}.local.", &own[..12]);
        let version = PROTOCOL_VERSION.to_string();
        let properties = [
            ("fp", own.as_str()),
            ("name", name.0.as_str()),
            ("v", version.as_str()),
        ];
        let service = ServiceInfo::new(SERVICE_TYPE, &name.0, &host, "", port, &properties[..])
            .map_err(mdns)?
            .enable_addr_auto();
        let fullname = service.get_fullname().to_owned();
        daemon.register(service).map_err(mdns)?;
        let events = daemon.browse(SERVICE_TYPE).map_err(mdns)?;

        let found = Arc::new(Mutex::new(HashMap::new()));
        let sink = Arc::clone(&found);
        let skip = own.clone();
        thread::Builder::new()
            .name("deskbaton-mdns".into())
            .spawn(move || {
                for event in events.iter() {
                    let changed = match event {
                        ServiceEvent::ServiceResolved(service) => {
                            let Some(device) = parse(
                                service.get_property_val_str("name"),
                                service.get_property_val_str("fp"),
                                service.get_property_val_str("v"),
                                service.get_addresses().iter().map(|ip| ip.to_ip_addr()),
                                service.get_port(),
                            )
                            .filter(|d| d.fingerprint != skip) else {
                                continue;
                            };
                            let mut found = lock(&sink);
                            let fresh = found.get(service.get_fullname()) != Some(&device);
                            found.insert(service.get_fullname().to_owned(), device);
                            fresh
                        }
                        ServiceEvent::ServiceRemoved(_, fullname) => {
                            lock(&sink).remove(&fullname).is_some()
                        }
                        _ => false,
                    };
                    if changed {
                        on_change();
                    }
                }
            })
            .map_err(|e| Error::Net(format!("无法启动 mDNS 线程：{e}")))?;
        Ok(Self {
            daemon,
            found,
            fingerprint: own,
            port,
            fullname: Mutex::new(fullname),
        })
    }

    /// 注销旧实例并以新名称重新注册；浏览线程继续跑。
    pub fn set_name(&self, name: &DeviceId) -> Result<()> {
        let mdns = |e: mdns_sd::Error| Error::Net(format!("mDNS：{e}"));
        let old = lock(&self.fullname).clone();
        if let Err(e) = self.daemon.unregister(&old) {
            log::warn!("注销旧的 mDNS 名称失败：{e}");
        }
        let host = format!("deskbaton-{}.local.", &self.fingerprint[..12]);
        let version = PROTOCOL_VERSION.to_string();
        let properties = [
            ("fp", self.fingerprint.as_str()),
            ("name", name.0.as_str()),
            ("v", version.as_str()),
        ];
        let service =
            ServiceInfo::new(SERVICE_TYPE, &name.0, &host, "", self.port, &properties[..])
                .map_err(mdns)?
                .enable_addr_auto();
        let fullname = service.get_fullname().to_owned();
        self.daemon.register(service).map_err(mdns)?;
        *lock(&self.fullname) = fullname;
        Ok(())
    }

    pub fn list(&self) -> Vec<Discovered> {
        let mut list: Vec<Discovered> = lock(&self.found).values().cloned().collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    }

    pub fn shutdown(&self) {
        let _ = self.daemon.shutdown();
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// 由解析到的服务组装设备；缺名称或指纹、没有 IPv4 地址时忽略。
fn parse(
    name: Option<&str>,
    fingerprint: Option<&str>,
    version: Option<&str>,
    ips: impl Iterator<Item = IpAddr>,
    port: u16,
) -> Option<Discovered> {
    let fingerprint = fingerprint.filter(|fp| auth::parse_fingerprint(fp).is_some())?;
    let mut addresses: Vec<SocketAddr> = ips
        .filter(IpAddr::is_ipv4)
        .map(|ip| SocketAddr::new(ip, port))
        .collect();
    addresses.sort();
    addresses.dedup();
    if addresses.is_empty() {
        return None;
    }
    Some(Discovered {
        name: DeviceId(name.filter(|n| !n.is_empty())?.to_owned()),
        fingerprint: fingerprint.to_owned(),
        addresses,
        version: version.and_then(|v| v.parse().ok()),
    })
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ips(list: &[&str]) -> impl Iterator<Item = IpAddr> {
        list.iter()
            .map(|s| s.parse().unwrap())
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn parses_ipv4_addresses_with_port() {
        let fp = auth::to_hex(&[1; 32]);
        let device = parse(
            Some("win"),
            Some(&fp),
            Some("5"),
            ips(&["192.168.1.20", "fe80::1", "192.168.1.20"]),
            47800,
        )
        .unwrap();
        assert_eq!(device.name.0, "win");
        assert_eq!(device.addresses, ["192.168.1.20:47800".parse().unwrap()]);
        assert_eq!(device.fingerprint(), Some([1; 32]));
        assert_eq!(device.version, Some(5));
    }

    #[test]
    fn ignores_incomplete_services() {
        let fp = auth::to_hex(&[1; 32]);
        assert!(parse(None, Some(&fp), None, ips(&["192.168.1.20"]), 1).is_none());
        assert!(parse(Some("win"), Some("zz"), None, ips(&["192.168.1.20"]), 1).is_none());
        assert!(parse(Some("win"), Some(&fp), None, ips(&["fe80::1"]), 1).is_none());
        let unversioned = parse(Some("win"), Some(&fp), None, ips(&["192.168.1.20"]), 1);
        assert_eq!(unversioned.unwrap().version, None);
    }
}
