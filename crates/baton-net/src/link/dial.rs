//! 主动拨号：每 2 秒检查一次档案里（有令牌时还有配置里）未连接的设备，依次尝试它的地址；
//! 有配对界面时，还拨 mDNS 发现的未配对设备，向双方弹出配对请求。

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use baton_core::ids::DeviceId;
use baton_core::protocol::PROTOCOL_VERSION;
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
    /// 未配对、为发起配对而拨的设备。
    pub unpaired: bool,
}

/// 除档案与配置外，还要不要拨 mDNS 发现的未配对设备。
#[derive(Debug, Default)]
pub(super) struct AutoPair<'a> {
    pub enabled: bool,
    pub me: Option<&'a DeviceId>,
    /// 本次运行中被拒绝、配对未完成过的指纹。
    pub skip: HashSet<Fingerprint>,
}

/// 档案里的设备：依次试 mDNS 按指纹解析到的地址、最后已知地址、配置地址。
/// 有令牌时，配置里未配对的对端也拨。`auto` 开启时，名称比本机大的未配对设备也拨，
/// 只由名称较小的一方发起，免得两边各开一个配对会话。
pub(super) fn targets(
    paired: &[PairedPeer],
    discovered: &[Discovered],
    configured: &[PeerAddr],
    token: bool,
    auto: &AutoPair<'_>,
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
                unpaired: false,
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
                    unpaired: true,
                });
            }
        }
    }
    if let (true, Some(me)) = (auto.enabled, auto.me) {
        for d in discovered {
            let Some(fingerprint) = auth::parse_fingerprint(&d.fingerprint) else {
                continue;
            };
            let known = paired.iter().any(|p| p.fingerprint == d.fingerprint)
                || out.iter().any(|t| t.name == d.name);
            // 协议版本不同的握手必然失败，自动拨只会反复报错；用户点「连接」时才提示版本不一致。
            let incompatible = d.version != Some(PROTOCOL_VERSION);
            if known
                || incompatible
                || d.name <= *me
                || auto.skip.contains(&fingerprint)
                || d.addresses.is_empty()
            {
                continue;
            }
            out.push(DialTarget {
                name: d.name.clone(),
                fingerprint: Some(fingerprint),
                addresses: d.addresses.iter().map(SocketAddr::to_string).collect(),
                unpaired: true,
            });
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
        let me = shared.local_id();
        let auto = AutoPair {
            enabled: shared.observer.is_some(),
            me: Some(&me),
            skip: lock(&shared.rejected)
                .iter()
                .chain(lock(&shared.auto_paused).iter())
                .copied()
                .collect(),
        };
        let targets = targets(
            &shared.store.list(),
            &shared.discovered(),
            &shared.config_peers,
            shared.token.is_some(),
            &auto,
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
    if shared.local_id() > target.name {
        // 名称较大的一方晚一点拨号，减少两端同时互拨。
        tokio::time::sleep(REDIAL_INTERVAL / 2).await;
    }
    let paired_session = Arc::new(AtomicBool::new(false));
    let intent = Intent {
        expect_name: target.fingerprint.is_none().then(|| target.name.clone()),
        expect_fp: target.fingerprint,
        initiated_by_user: false,
        reported: Some(Arc::clone(&paired_session)),
    };
    for address in &target.addresses {
        if shared.is_connected(&target.name) {
            break;
        }
        match dial_once(&shared, address, intent.clone()).await {
            Ok(()) => break,
            Err(e) => {
                log::debug!("连接 {}（{address}）失败：{e}", target.name.0);
                // 弹过配对请求却没配成（拒绝、超时、对方拒绝）：本次运行不再自动拨号，免得反复弹窗。
                if target.unpaired && paired_session.load(Ordering::SeqCst) {
                    if let Some(fp) = target.fingerprint {
                        lock(&shared.auto_paused).insert(fp);
                    }
                    break;
                }
            }
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
        Vec::new(),
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
            targets(&[peer], &[], &config, false, &AutoPair::default()),
            [DialTarget {
                name: DeviceId("win".into()),
                fingerprint: Some(A),
                addresses: vec!["192.168.1.20:47800".into(), "win.local:47800".into()],
                unpaired: false,
            }]
        );
    }

    #[test]
    fn unpaired_config_peers_are_dialed_only_with_token() {
        let config = [configured("win", "192.168.1.20:47800")];
        assert!(targets(&[], &[], &config, false, &AutoPair::default()).is_empty());
        assert_eq!(
            targets(&[], &[], &config, true, &AutoPair::default()),
            [DialTarget {
                name: DeviceId("win".into()),
                fingerprint: None,
                addresses: vec!["192.168.1.20:47800".into()],
                unpaired: true,
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
                version: Some(PROTOCOL_VERSION),
            },
            Discovered {
                name: DeviceId("win".into()),
                fingerprint: auth::to_hex(&[9; 32]),
                addresses: vec!["192.168.1.40:47800".parse().unwrap()],
                version: Some(PROTOCOL_VERSION),
            },
        ];
        assert_eq!(
            targets(&[peer], &discovered, &[], false, &AutoPair::default())[0].addresses,
            ["192.168.1.30:47800", "192.168.1.20:47800"]
        );
    }

    #[test]
    fn paired_peer_without_address_is_skipped() {
        assert!(
            targets(
                &[PairedPeer::new("win", &A)],
                &[],
                &[],
                true,
                &AutoPair::default()
            )
            .is_empty()
        );
    }

    fn found(name: &str, fp: &Fingerprint, address: &str) -> Discovered {
        Discovered {
            name: DeviceId(name.into()),
            fingerprint: auth::to_hex(fp),
            addresses: vec![address.parse().unwrap()],
            version: Some(PROTOCOL_VERSION),
        }
    }

    #[test]
    fn unpaired_discovered_devices_are_dialed_by_the_smaller_name_with_ui() {
        const B: Fingerprint = [2; 32];
        const C: Fingerprint = [3; 32];
        let me = DeviceId("mac".into());
        let discovered = [
            found("win", &A, "192.168.1.20:47800"),
            found("aaa", &B, "192.168.1.21:47800"),
            found("zzz", &C, "192.168.1.22:47800"),
        ];
        let auto = AutoPair {
            enabled: true,
            me: Some(&me),
            skip: HashSet::from([C]),
        };
        assert_eq!(
            targets(&[], &discovered, &[], false, &auto),
            [DialTarget {
                name: DeviceId("win".into()),
                fingerprint: Some(A),
                addresses: vec!["192.168.1.20:47800".into()],
                unpaired: true,
            }],
            "名称比本机小的由对方发起，跳过名单里的不拨"
        );

        let headless = AutoPair {
            enabled: false,
            me: Some(&me),
            skip: HashSet::new(),
        };
        assert!(targets(&[], &discovered, &[], false, &headless).is_empty());

        let paired = PairedPeer::new("win", &A);
        let only_paired = targets(&[paired], &discovered, &[], false, &auto);
        assert_eq!(only_paired.len(), 1);
        assert!(!only_paired[0].unpaired);

        let old = Discovered {
            version: Some(PROTOCOL_VERSION - 1),
            ..found("win", &A, "192.168.1.20:47800")
        };
        assert!(
            targets(&[], &[old], &[], false, &auto).is_empty(),
            "协议版本不同的设备不自动拨"
        );
    }
}
