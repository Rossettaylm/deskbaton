//! 文件连接的两端：拨号方出示 `file_hello` 后只写数据；被拨方校验已配对后把帧交给文件会话。
//! 这条连接不登记为对端连接，不参与上线、离线判定。

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use baton_core::files::SessionId;
use baton_core::protocol::{Message, PROTOCOL_VERSION};
use baton_core::{Error, Result};
use rustls::pki_types::ServerName;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_rustls::TlsConnector;

use super::{
    CONNECT_TIMEOUT, HANDSHAKE_TIMEOUT, Shared, io, peer_fingerprint, read_frame, write_frame,
};
use crate::auth::{self, Fingerprint};
use crate::file::{FileDownload, FileFrame, FileUpload};

/// 读写任意一帧的最长等待；对端停住超过这么久即判为失败。
const FILE_IDLE: Duration = Duration::from_secs(30);
/// 两端之间缓冲的帧数，约 4 MiB。
const IN_FLIGHT: usize = 16;

/// 被拨方：首帧已确认为 `file_hello`。
pub(super) async fn serve_incoming<S>(
    shared: Arc<Shared>,
    mut stream: S,
    mut buf: Vec<u8>,
    peer_fp: Fingerprint,
    hello: Message,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let Message::FileHello {
        protocol_version,
        session_id,
        token,
    } = hello
    else {
        return Err(Error::Net("文件连接没有先发送 file_hello".into()));
    };
    if protocol_version != PROTOCOL_VERSION {
        return Err(Error::Net(format!(
            "文件连接的协议版本为 {protocol_version}，本机为 {PROTOCOL_VERSION}"
        )));
    }
    let peer = shared
        .store
        .find(&peer_fp)
        .ok_or_else(|| Error::Net("未配对的设备发来文件连接".into()))?;
    let handler = shared
        .files
        .get()
        .ok_or_else(|| Error::Net("本机不接收文件".into()))?;
    let (tx, rx) = mpsc::channel(IN_FLIGHT);
    handler.incoming(peer.device_id(), session_id, token, FileDownload { rx });
    loop {
        let read = timeout(FILE_IDLE, read_frame(&mut stream, &mut buf)).await;
        let item = match read {
            Ok(Ok(frame)) => FileFrame::from_frame(frame),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(Error::Net("文件连接超时".into())),
        };
        let end = !matches!(
            item,
            Ok(FileFrame::Manifest(_)
                | FileFrame::Begin(_)
                | FileFrame::Chunk(_)
                | FileFrame::End { .. })
        );
        if tx.send(item).await.is_err() || end {
            let _ = stream.shutdown().await;
            return Ok(());
        }
    }
}

/// 拨号方：连到 `addr`，核对证书指纹，发 `file_hello`，之后由写任务把排队的帧写出。
pub(super) async fn dial(
    shared: Arc<Shared>,
    addr: SocketAddr,
    expect_fp: Fingerprint,
    session_id: SessionId,
    token: &str,
) -> Result<FileUpload> {
    let tcp = timeout(CONNECT_TIMEOUT, TcpStream::connect(addr))
        .await
        .map_err(|_| Error::Net(format!("连接 {addr} 超时")))?
        .map_err(io)?;
    let _ = tcp.set_nodelay(true);
    let connector = TlsConnector::from(Arc::clone(&shared.identity.client));
    let name = ServerName::try_from(auth::SERVER_NAME).map_err(|e| Error::Net(e.to_string()))?;
    let mut tls = timeout(HANDSHAKE_TIMEOUT, connector.connect(name, tcp))
        .await
        .map_err(|_| Error::Net("TLS 握手超时".into()))?
        .map_err(io)?;
    if peer_fingerprint(tls.get_ref().1.peer_certificates())? != expect_fp {
        return Err(Error::Net("文件连接的对端与已配对设备不符".into()));
    }
    write_frame(
        &mut tls,
        &Message::FileHello {
            protocol_version: PROTOCOL_VERSION,
            session_id,
            token: token.to_owned(),
        },
    )
    .await?;
    let (tx, mut rx) = mpsc::channel::<Vec<crate::frame::Frame>>(IN_FLIGHT);
    let (done_tx, done) = std::sync::mpsc::channel();
    tokio::spawn(async move {
        let result = async {
            while let Some(frames) = rx.recv().await {
                for frame in frames {
                    timeout(FILE_IDLE, tls.write_all(&frame.encode()?))
                        .await
                        .map_err(|_| Error::Net("文件连接写入超时".into()))?
                        .map_err(io)?;
                }
            }
            tls.flush().await.map_err(io)?;
            tls.shutdown().await.map_err(io)
        }
        .await;
        rx.close();
        let _ = done_tx.send(result);
    });
    Ok(FileUpload { tx, done })
}
