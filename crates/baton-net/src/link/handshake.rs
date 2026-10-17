//! 握手 v3：hello、配对码随机数交换、信任判定、等待双方 `pair_accept`。
//! 流程见 `docs/superpowers/specs/2026-10-17-pairing-design.md` 第 4、5 节。

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use baton_core::event::PeerInfo;
use baton_core::ids::DeviceId;
use baton_core::protocol::{Message, PROTOCOL_VERSION};
use baton_core::{Error, Result};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::oneshot;
use tokio::time::{Instant, timeout, timeout_at};

use super::{
    HANDSHAKE_TIMEOUT, PAIRING_TIMEOUT, PairingSession, Session, Shared, lock, read_frame,
    write_frame,
};
use crate::auth::{self, Fingerprint, Role};
use crate::pairing::{self, Trust, TrustInput};
use crate::store::PairedPeer;

/// 这次连接期望连到谁、是不是用户主动发起的配对。
#[derive(Debug, Clone, Default)]
pub(super) struct Intent {
    pub expect_name: Option<DeviceId>,
    pub expect_fp: Option<Fingerprint>,
    pub initiated_by_user: bool,
}

struct PeerHello {
    info: PeerInfo,
    listen_port: u16,
    token_ok: bool,
}

/// 完成握手并返回对端信息；双方都发出并收到 `pair_accept` 才返回 `Ok`。
#[allow(clippy::too_many_arguments)]
pub(super) async fn run<R, W>(
    shared: &Arc<Shared>,
    reader: &mut R,
    writer: &mut W,
    buf: &mut Vec<u8>,
    role: Role,
    peer_fp: Fingerprint,
    peer_addr: SocketAddr,
    intent: Intent,
) -> Result<PeerInfo>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let own_fp = shared.identity.fingerprint;
    let (client_fp, server_fp) = match role {
        Role::Client => (own_fp, peer_fp),
        Role::Server => (peer_fp, own_fp),
    };
    let nonce = pairing::new_nonce()?;
    let (hello, code) = match role {
        Role::Client => {
            write_frame(writer, &hello(shared, role, &client_fp, &server_fp)?).await?;
            write_frame(
                writer,
                &Message::PairCommit {
                    commit: auth::to_hex(&pairing::commit(&nonce)),
                },
            )
            .await?;
            let hello = read_message(reader, buf).await?;
            let Message::PairNonce {
                nonce: server_nonce,
            } = read_message(reader, buf).await?
            else {
                return Err(Error::Net("对端没有发送配对随机数".into()));
            };
            let server_nonce = parse_nonce(&server_nonce)?;
            write_frame(
                writer,
                &Message::PairReveal {
                    nonce: auth::to_hex(&nonce),
                },
            )
            .await?;
            let code = pairing::pairing_code(&client_fp, &server_fp, &nonce, &server_nonce);
            (hello, code)
        }
        Role::Server => {
            let hello = read_message(reader, buf).await?;
            let Message::PairCommit { commit } = read_message(reader, buf).await? else {
                return Err(Error::Net("对端没有发送配对承诺".into()));
            };
            write_frame(writer, &self::hello(shared, role, &client_fp, &server_fp)?).await?;
            write_frame(
                writer,
                &Message::PairNonce {
                    nonce: auth::to_hex(&nonce),
                },
            )
            .await?;
            let Message::PairReveal {
                nonce: client_nonce,
            } = read_message(reader, buf).await?
            else {
                return Err(Error::Net("对端没有公开配对随机数".into()));
            };
            let client_nonce = parse_nonce(&client_nonce)?;
            let commit = auth::from_hex(&commit).unwrap_or_default();
            if !pairing::verify_commit(&commit, &client_nonce) {
                return Err(Error::Net("配对承诺校验失败".into()));
            }
            let code = pairing::pairing_code(&client_fp, &server_fp, &client_nonce, &nonce);
            (hello, code)
        }
    };
    let PeerHello {
        info,
        listen_port,
        token_ok,
    } = check_hello(
        shared,
        hello,
        role.peer(),
        &client_fp,
        &server_fp,
        peer_addr,
    )?;
    let name = info.device_id.clone();

    let unexpected_peer = intent.expect_fp.is_some_and(|fp| fp != peer_fp)
        || intent.expect_name.as_ref().is_some_and(|n| *n != name);
    let rejected_before = lock(&shared.rejected).contains(&peer_fp);
    let trust = pairing::decide(TrustInput {
        unexpected_peer,
        pinned: shared.store.find(&peer_fp).is_some(),
        token_ok,
        interactive: shared.observer.is_some(),
        rejected_before,
        initiated_by_user: intent.initiated_by_user,
    });
    log::debug!("{} 的信任判定：{trust:?}", name.0);

    let mut session = None;
    let mut decision = None;
    let mut sent_accept = false;
    // 双方都确认后才写入档案，任一方拒绝都不留记录。
    let mut pin = false;
    let mut wait = HANDSHAKE_TIMEOUT;
    match trust {
        Trust::Pinned => {
            write_frame(writer, &Message::PairAccept).await?;
            sent_accept = true;
        }
        Trust::Token => {
            pin = true;
            write_frame(writer, &Message::PairAccept).await?;
            sent_accept = true;
        }
        Trust::AskUser => {
            let dialer = dialer(shared, role, &name);
            let (id, rx) = open_session(shared, &name, peer_fp, &code, dialer, true)?;
            session = Some(id);
            decision = Some(rx);
            wait = PAIRING_TIMEOUT;
            write_frame(writer, &Message::PairPending).await?;
            log::info!("{} 请求配对，配对码 {code}", name.0);
        }
        Trust::Deny => {
            let reason = if unexpected_peer {
                format!("对端 {} 与期望连接的设备不符", name.0)
            } else {
                format!(
                    "{} 未配对：请在 DeskBaton 面板中配对，或两端配置相同的 network.token",
                    name.0
                )
            };
            if !rejected_before && !unexpected_peer {
                log::info!("{reason}");
                if shared.observer.is_none() {
                    lock(&shared.rejected).insert(peer_fp);
                }
            }
            return Err(Error::Net(reason));
        }
    }

    let mut peer_accepted = false;
    let mut deadline = Instant::now() + wait;
    let result: Result<()> = async {
        while !(sent_accept && peer_accepted) {
            tokio::select! {
                read = timeout_at(deadline, read_frame(reader, buf)) => {
                    let frame = match read {
                        Ok(Ok(frame)) => frame,
                        Ok(Err(_)) if session.is_some() => {
                            return Err(Error::Net("对方拒绝或取消了配对".into()));
                        }
                        Ok(Err(e)) => return Err(e),
                        Err(_) if session.is_some() => {
                            return Err(Error::Net("配对超时".into()));
                        }
                        Err(_) => return Err(Error::Net("等待对端确认超时".into())),
                    };
                    match frame.to_message()? {
                        Message::PairAccept => peer_accepted = true,
                        Message::PairPending => {
                            deadline = Instant::now() + PAIRING_TIMEOUT;
                            if session.is_none() {
                                let dialer = dialer(shared, role, &name);
                                let (id, rx) =
                                    open_session(shared, &name, peer_fp, &code, dialer, false)?;
                                session = Some(id);
                                decision = Some(rx);
                            }
                        }
                        other => {
                            return Err(Error::Net(format!("握手阶段收到意外消息：{other:?}")));
                        }
                    }
                }
                decided = recv_decision(&mut decision) => {
                    decision = None;
                    match decided {
                        Some(true) => {
                            pin = true;
                            write_frame(writer, &Message::PairAccept).await?;
                            sent_accept = true;
                            if let Some(id) = session {
                                mark_waiting(shared, id);
                            }
                        }
                        Some(false) => {
                            lock(&shared.rejected).insert(peer_fp);
                            return Err(Error::Net("已拒绝配对".into()));
                        }
                        None => return Err(Error::Net("配对已取消".into())),
                    }
                }
            }
        }
        Ok(())
    }
    .await;

    let result = result.and_then(|()| {
        if pin {
            shared.store.upsert(PairedPeer::new(&name.0, &peer_fp))?;
            if session.is_none() {
                log::info!("已通过预共享令牌与 {} 配对", name.0);
            }
        }
        Ok(())
    });
    if let Some(id) = session {
        close_session(shared, id);
        match &result {
            Ok(()) => shared.notice(&format!("已与 {} 配对", name.0)),
            Err(e) => shared.notice(&format!("与 {} 的配对未完成：{e}", name.0)),
        }
    }
    result?;
    if let Err(e) = shared.store.refresh(&peer_fp, &info, Some(listen_port)) {
        log::warn!("更新设备档案失败：{e}");
    }
    Ok(info)
}

fn hello(
    shared: &Shared,
    role: Role,
    client_fp: &Fingerprint,
    server_fp: &Fingerprint,
) -> Result<Message> {
    let auth = shared
        .token
        .as_ref()
        .map(|token| auth::proof(token, role, &shared.me.device_id, client_fp, server_fp))
        .transpose()?
        .map(|proof| auth::to_hex(&proof));
    Ok(Message::Hello {
        device_id: shared.me.device_id.clone(),
        protocol_version: PROTOCOL_VERSION,
        input_code: shared.me.input_code,
        wol_mac: shared.me.wol_mac,
        listen_port: shared.listen_port,
        auth,
    })
}

fn check_hello(
    shared: &Shared,
    message: Message,
    peer_role: Role,
    client_fp: &Fingerprint,
    server_fp: &Fingerprint,
    peer_addr: SocketAddr,
) -> Result<PeerHello> {
    let Message::Hello {
        device_id,
        protocol_version,
        input_code,
        wol_mac,
        listen_port,
        auth,
    } = message
    else {
        return Err(Error::Net("对端没有先发送 hello".into()));
    };
    if protocol_version != PROTOCOL_VERSION {
        log::warn!(
            "{} 的协议版本为 {protocol_version}，本机为 {PROTOCOL_VERSION}，请两端同时更新",
            device_id.0
        );
        return Err(Error::Net(format!(
            "协议版本不一致：本机 {PROTOCOL_VERSION}，{} 为 {protocol_version}",
            device_id.0
        )));
    }
    if device_id == shared.me.device_id {
        return Err(Error::Net("对端名称与本机相同".into()));
    }
    let token_ok = match (&shared.token, auth) {
        (Some(token), Some(claimed)) => auth::verify(
            token,
            peer_role,
            &device_id,
            client_fp,
            server_fp,
            &auth::from_hex(&claimed).unwrap_or_default(),
        ),
        _ => false,
    };
    Ok(PeerHello {
        info: PeerInfo {
            device_id,
            input_code,
            wol_mac,
            ip: (!peer_addr.ip().is_loopback()).then_some(peer_addr.ip()),
        },
        listen_port,
        token_ok,
    })
}

async fn read_message<R: AsyncRead + Unpin>(reader: &mut R, buf: &mut Vec<u8>) -> Result<Message> {
    timeout(HANDSHAKE_TIMEOUT, read_frame(reader, buf))
        .await
        .map_err(|_| Error::Net("握手超时".into()))??
        .to_message()
}

fn parse_nonce(hex: &str) -> Result<Vec<u8>> {
    auth::from_hex(hex)
        .filter(|n| n.len() == 32)
        .ok_or_else(|| Error::Net("配对随机数格式不对".into()))
}

fn dialer(shared: &Shared, role: Role, peer: &DeviceId) -> DeviceId {
    match role {
        Role::Client => shared.me.device_id.clone(),
        Role::Server => peer.clone(),
    }
}

async fn recv_decision(rx: &mut Option<oneshot::Receiver<bool>>) -> Option<bool> {
    match rx {
        Some(rx) => rx.await.ok(),
        None => std::future::pending().await,
    }
}

/// 同一对端同时有两个配对会话时，保留由名称较小一方拨出的那个，两端结果一致。
fn open_session(
    shared: &Shared,
    peer: &DeviceId,
    fingerprint: Fingerprint,
    code: &str,
    dialer: DeviceId,
    awaiting_me: bool,
) -> Result<(u64, oneshot::Receiver<bool>)> {
    let preferred = shared.preferred_dialer(peer);
    let mut sessions = lock(&shared.sessions);
    let clashing: Vec<u64> = sessions
        .iter()
        .filter(|(_, s)| s.fingerprint == fingerprint)
        .map(|(id, _)| *id)
        .collect();
    for id in &clashing {
        if sessions[id].dialer == preferred || dialer != preferred {
            return Err(Error::Net(format!("与 {} 的配对已在进行", peer.0)));
        }
    }
    for id in clashing {
        sessions.remove(&id);
    }
    let id = shared.next_session.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = oneshot::channel();
    sessions.insert(
        id,
        Session {
            view: PairingSession {
                id,
                peer: peer.clone(),
                fingerprint: auth::to_hex(&fingerprint),
                code: code.to_owned(),
                awaiting_me,
            },
            fingerprint,
            dialer,
            decide: Some(tx),
        },
    );
    drop(sessions);
    shared.changed();
    Ok((id, rx))
}

fn mark_waiting(shared: &Shared, id: u64) {
    if let Some(session) = lock(&shared.sessions).get_mut(&id) {
        session.view.awaiting_me = false;
    }
    shared.changed();
}

fn close_session(shared: &Shared, id: u64) {
    let removed = lock(&shared.sessions).remove(&id).is_some();
    if removed {
        shared.changed();
    }
}
