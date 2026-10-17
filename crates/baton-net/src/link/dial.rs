//! 主动拨号：每 2 秒检查一次档案里（有令牌时还有配置里）未连接的设备，依次尝试它的地址。

use std::net::SocketAddr;
use std::sync::Arc;

use baton_core::ids::DeviceId;
use baton_core::{Error, Result};
use rustls::pki_types::ServerName;
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_rustls::TlsConnector;

use super::handshake::Intent;
use super::{
    CONNECT_TIMEOUT, HANDSHAKE_TIMEOUT, PeerAddr, REDIAL_INTERVAL, Shared, io, lock,
    peer_fingerprint, serve,
};
use crate::auth::{self, Fingerprint, Role};
use crate::discovery::Discovered;
use crate::store::PairedPeer;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DialTarget {
    pub name: DeviceId,
    /// 已配对设备按指纹认人；配置里未配对的对端按名称认人。
    pub fingerprint: Option<Fingerprint>,
    /// 按顺序尝试，第一个连上即止。
    pub addresses: Vec<String>,
}

/// 档案里的设备：依次试 mDNS 按指纹解析到的地址、最后已知地址、配置地址。
/// 有令牌时，配置里未配对的对端也拨。
pub(super) fn targets(
    paired: &[PairedPeer],
    discovered: &[Discovered],
    configured: &[PeerAddr],
    token: bool,
) -> Vec<DialTarget> {
    let configured_address = |name: &str| -> Vec<String> {
        configured
            .iter()
            .filter(|c| c.device_id.0 == name)
            .map(|c| c.address.clone())
            .collect()
    };
    let mut out = Vec::new();
    for peer in paired {
        let Some(fingerprint) = auth::parse_fingerprint(&peer.fingerprint) else {
            continue;
        };
        let mut addresses: Vec<String> = discovered
            .iter()
            .filter(|d| d.fingerprint == peer.fingerprint)
            .flat_map(|d| d.addresses.iter().map(SocketAddr::to_string))
            .chain(
                peer.last_ip
                    .zip(peer.port)
                    .map(|(ip, port)| SocketAddr::new(ip, port).to_string()),
            )
            .chain(configured_address(&peer.name))
            .collect();
        dedup(&mut addresses);
        if !addresses.is_empty() {
            out.push(DialTarget {
                name: peer.device_id(),
                fingerprint: Some(fingerprint),
                addresses,
            });
        }
    }
    if token {
        for c in configured {
            if !paired.iter().any(|p| p.name == c.device_id.0) {
                out.push(DialTarget {
                    name: c.device_id.clone(),
                    fingerprint: None,
                    addresses: vec![c.address.clone()],
                });
            }
        }
    }
    out
}

fn dedup(addresses: &mut Vec<String>) {
    let mut seen = Vec::new();
    addresses.retain(|a| {
        let fresh = !seen.contains(a);
        seen.push(a.clone());
        fresh
    });
}

pub(super) async fn supervise(shared: Arc<Shared>) {
    loop {
        let targets = targets(
            &shared.store.list(),
            &shared.discovered(),
            &shared.config_peers,
            shared.token.is_some(),
        );
        for target in targets {
            if shared.is_connected(&target.name)
                || !lock(&shared.dialing).insert(target.name.clone())
            {
                continue;
            }
            tokio::spawn(dial_target(Arc::clone(&shared), target));
        }
        tokio::time::sleep(REDIAL_INTERVAL).await;
    }
}

async fn dial_target(shared: Arc<Shared>, target: DialTarget) {
    if shared.me.device_id > target.name {
        // 名称较大的一方晚一点拨号，减少两端同时互拨。
        tokio::time::sleep(REDIAL_INTERVAL / 2).await;
    }
    let intent = Intent {
        expect_name: target.fingerprint.is_none().then(|| target.name.clone()),
        expect_fp: target.fingerprint,
        initiated_by_user: false,
        reported: None,
    };
    for address in &target.addresses {
        if shared.is_connected(&target.name) {
            break;
        }
        match dial_once(&shared, address, intent.clone()).await {
            Ok(()) => break,
            Err(e) => log::debug!("连接 {}（{address}）失败：{e}", target.name.0),
        }
    }
    lock(&shared.dialing).remove(&target.name);
}

/// 拨一次号；连接建立后一直运行到断开才返回 `Ok`。
pub(super) async fn dial_once(shared: &Arc<Shared>, address: &str, intent: Intent) -> Result<()> {
    let tcp = timeout(CONNECT_TIMEOUT, TcpStream::connect(address))
        .await
        .map_err(|_| Error::Net("连接超时".into()))?
        .map_err(io)?;
    let _ = tcp.set_nodelay(true);
    let addrs = (tcp.local_addr().map_err(io)?, tcp.peer_addr().map_err(io)?);
    let connector = TlsConnector::from(Arc::clone(&shared.identity.client));
    let name = ServerName::try_from(auth::SERVER_NAME).map_err(|e| Error::Net(e.to_string()))?;
    let tls = timeout(HANDSHAKE_TIMEOUT, connector.connect(name, tcp))
        .await
        .map_err(|_| Error::Net("TLS 握手超时".into()))?
        .map_err(io)?;
    let peer_fp = peer_fingerprint(tls.get_ref().1.peer_certificates())?;
    serve(
        Arc::clone(shared),
        tls,
        Role::Client,
        peer_fp,
        addrs,
        intent,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: Fingerprint = [1; 32];

    fn configured(name: &str, address: &str) -> PeerAddr {
        PeerAddr {
            device_id: DeviceId(name.into()),
            address: address.into(),
        }
    }

    #[test]
    fn paired_peers_try_last_address_then_config() {
        let mut peer = PairedPeer::new("win", &A);
        peer.last_ip = Some("192.168.1.20".parse().unwrap());
        peer.port = Some(47800);
        let config = [
            configured("win", "win.local:47800"),
            configured("win", "192.168.1.20:47800"),
        ];
        assert_eq!(
            targets(&[peer], &[], &config, false),
            [DialTarget {
                name: DeviceId("win".into()),
                fingerprint: Some(A),
                addresses: vec!["192.168.1.20:47800".into(), "win.local:47800".into()],
            }]
        );
    }

    #[test]
    fn unpaired_config_peers_are_dialed_only_with_token() {
        let config = [configured("win", "192.168.1.20:47800")];
        assert!(targets(&[], &[], &config, false).is_empty());
        assert_eq!(
            targets(&[], &[], &config, true),
            [DialTarget {
                name: DeviceId("win".into()),
                fingerprint: None,
                addresses: vec!["192.168.1.20:47800".into()],
            }]
        );
    }

    #[test]
    fn discovered_address_comes_first() {
        let mut peer = PairedPeer::new("win", &A);
        peer.last_ip = Some("192.168.1.20".parse().unwrap());
        peer.port = Some(47800);
        let discovered = [
            Discovered {
                name: DeviceId("win".into()),
                fingerprint: auth::to_hex(&A),
                addresses: vec!["192.168.1.30:47800".parse().unwrap()],
            },
            Discovered {
                name: DeviceId("win".into()),
                fingerprint: auth::to_hex(&[9; 32]),
                addresses: vec!["192.168.1.40:47800".parse().unwrap()],
            },
        ];
        assert_eq!(
            targets(&[peer], &discovered, &[], false)[0].addresses,
            ["192.168.1.30:47800", "192.168.1.20:47800"]
        );
    }

    #[test]
    fn paired_peer_without_address_is_skipped() {
        assert!(targets(&[PairedPeer::new("win", &A)], &[], &[], true).is_empty());
    }
}
