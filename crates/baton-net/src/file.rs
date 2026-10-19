//! 文件连接：同意后另拨的一条 TLS 连接，只服务一个会话，数据走通道 2。
//! 格式见 `docs/superpowers/specs/2026-10-19-file-transfer-design.md` 第 5.2 节。

use baton_core::channel::ChannelId;
use baton_core::files::{Entry, SessionId};
use baton_core::ids::DeviceId;
use baton_core::protocol::Message;
use baton_core::{Error, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::frame::Frame;

pub const KIND_MANIFEST: u8 = 0;
pub const KIND_BEGIN: u8 = 1;
pub const KIND_CHUNK: u8 = 2;
pub const KIND_END: u8 = 3;
pub const KIND_DONE: u8 = 4;

/// 每个 Chunk 帧携带的数据量。
pub const FILE_CHUNK_LEN: usize = 256 * 1024;
/// 一个 Manifest 帧里条目 JSON 的大致上限，远低于单帧 1 MiB。
const MANIFEST_BATCH: usize = 512 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileFrame {
    /// 清单的一段，多段依次拼接。
    Manifest(Vec<Entry>),
    /// 开始发送清单中第 n 项（从 0 计）。
    Begin(u64),
    Chunk(Vec<u8>),
    /// 当前文件发完；`sha256` 为空表示发送方读取失败，接收方丢弃该文件。
    End {
        sha256: Option<String>,
    },
    Done,
}

#[derive(Serialize, Deserialize)]
struct EndBody {
    sha256: Option<String>,
}

impl FileFrame {
    pub fn into_frames(self) -> Result<Vec<Frame>> {
        let frame = |kind, payload| Frame {
            channel: ChannelId::FILE,
            kind,
            payload,
        };
        Ok(match self {
            Self::Manifest(entries) => {
                let mut frames = Vec::new();
                let mut batch: Vec<Entry> = Vec::new();
                let mut size = 0;
                for entry in entries {
                    let len = json(&entry)?.len() + 1;
                    if !batch.is_empty() && size + len > MANIFEST_BATCH {
                        frames.push(frame(KIND_MANIFEST, json(&batch)?));
                        batch.clear();
                        size = 0;
                    }
                    size += len;
                    batch.push(entry);
                }
                if !batch.is_empty() || frames.is_empty() {
                    frames.push(frame(KIND_MANIFEST, json(&batch)?));
                }
                frames
            }
            Self::Begin(index) => vec![frame(KIND_BEGIN, index.to_be_bytes().to_vec())],
            Self::Chunk(data) => vec![frame(KIND_CHUNK, data)],
            Self::End { sha256 } => vec![frame(KIND_END, json(&EndBody { sha256 })?)],
            Self::Done => vec![frame(KIND_DONE, Vec::new())],
        })
    }

    pub fn from_frame(frame: Frame) -> Result<Self> {
        if frame.channel != ChannelId::FILE {
            return Err(Error::Net(format!(
                "文件连接上出现通道 {}",
                frame.channel.0
            )));
        }
        match frame.kind {
            KIND_MANIFEST => Ok(Self::Manifest(parse(&frame.payload)?)),
            KIND_BEGIN => {
                let bytes: [u8; 8] = frame
                    .payload
                    .as_slice()
                    .try_into()
                    .map_err(|_| Error::Net("Begin 帧长度不对".into()))?;
                Ok(Self::Begin(u64::from_be_bytes(bytes)))
            }
            KIND_CHUNK => Ok(Self::Chunk(frame.payload)),
            KIND_END => Ok(Self::End {
                sha256: parse::<EndBody>(&frame.payload)?.sha256,
            }),
            KIND_DONE => Ok(Self::Done),
            other => Err(Error::Net(format!("未知的文件帧类型 {other}"))),
        }
    }
}

fn json<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|e| Error::Net(e.to_string()))
}

fn parse<'a, T: Deserialize<'a>>(bytes: &'a [u8]) -> Result<T> {
    serde_json::from_slice(bytes).map_err(|e| Error::Net(e.to_string()))
}

/// 发送端句柄。方法会阻塞，只能在普通线程里调用，不能在异步运行时内。
pub struct FileUpload {
    pub(crate) tx: mpsc::Sender<Vec<Frame>>,
    pub(crate) done: std::sync::mpsc::Receiver<Result<()>>,
}

impl FileUpload {
    /// 排队发送；对端读得慢时阻塞，连接断开时报错。
    pub fn send(&self, frame: FileFrame) -> Result<()> {
        self.tx
            .blocking_send(frame.into_frames()?)
            .map_err(|_| self.closed())
    }

    /// 等全部数据写出、连接关闭。
    pub fn finish(self) -> Result<()> {
        let Self { tx, done } = self;
        drop(tx);
        done.recv()
            .unwrap_or_else(|_| Err(Error::Net("文件连接已断开".into())))
    }

    fn closed(&self) -> Error {
        match self.done.try_recv() {
            Ok(Err(e)) => e,
            _ => Error::Net("文件连接已断开".into()),
        }
    }
}

/// 接收端句柄。丢弃即断开连接。
pub struct FileDownload {
    pub(crate) rx: mpsc::Receiver<Result<FileFrame>>,
}

impl FileDownload {
    /// 阻塞到下一帧；连接结束时为 `None`。只能在普通线程里调用。
    pub fn recv(&mut self) -> Option<Result<FileFrame>> {
        self.rx.blocking_recv()
    }
}

/// 文件会话：接收 `file_*` 控制消息和对端拨来的文件连接。各方法在网络线程上调用，不得阻塞。
pub trait FileHandler: Send + Sync {
    fn message(&self, from: &DeviceId, message: Message);
    /// 已配对设备拨来的文件连接；令牌由实现校验，不符时丢弃 `download`。
    fn incoming(
        &self,
        from: DeviceId,
        session_id: SessionId,
        token: String,
        download: FileDownload,
    );
    /// 与该对端的长连接断开。
    fn peer_offline(&self, peer: &DeviceId);
}

#[cfg(test)]
mod tests {
    use baton_core::files::EntryKind;

    use super::*;
    use crate::frame;

    fn entry(name: String) -> Entry {
        Entry {
            path: vec![name],
            kind: EntryKind::File,
            size: 1,
        }
    }

    fn roundtrip(frame: FileFrame) -> Vec<FileFrame> {
        frame
            .into_frames()
            .unwrap()
            .into_iter()
            .map(|f| {
                let bytes = f.encode().unwrap();
                let (decoded, _) = frame::decode(&bytes).unwrap().unwrap();
                FileFrame::from_frame(decoded).unwrap()
            })
            .collect()
    }

    #[test]
    fn frames_roundtrip() {
        for f in [
            FileFrame::Begin(7),
            FileFrame::Chunk(vec![1, 2, 3]),
            FileFrame::End {
                sha256: Some("ab".into()),
            },
            FileFrame::End { sha256: None },
            FileFrame::Done,
            FileFrame::Manifest(Vec::new()),
        ] {
            assert_eq!(roundtrip(f.clone()), [f]);
        }
    }

    #[test]
    fn large_manifest_splits_into_frames_under_limit() {
        let entries: Vec<Entry> = (0..20_000)
            .map(|i| entry(format!("文件-{i:06}.txt")))
            .collect();
        let frames = FileFrame::Manifest(entries.clone()).into_frames().unwrap();
        assert!(frames.len() > 1);
        assert!(frames.iter().all(|f| f.encode().is_ok()));
        let joined: Vec<Entry> = roundtrip(FileFrame::Manifest(entries.clone()))
            .into_iter()
            .flat_map(|f| match f {
                FileFrame::Manifest(part) => part,
                _ => Vec::new(),
            })
            .collect();
        assert_eq!(joined, entries);
    }

    #[test]
    fn wire_format_is_stable() {
        let frames = FileFrame::Begin(1).into_frames().unwrap();
        assert_eq!(
            frames[0].encode().unwrap(),
            [0, 0, 0, 10, 2, 1, 0, 0, 0, 0, 0, 0, 0, 1]
        );
        let end = FileFrame::End { sha256: None }.into_frames().unwrap();
        assert_eq!(end[0].payload, br#"{"sha256":null}"#);
        let manifest = FileFrame::Manifest(vec![entry("a".into())])
            .into_frames()
            .unwrap();
        assert_eq!(
            manifest[0].payload,
            br#"[{"path":["a"],"kind":"file","size":1}]"#
        );
    }

    #[test]
    fn rejects_foreign_channel_and_kind() {
        let wrong_channel = Frame {
            channel: ChannelId::CLIPBOARD,
            kind: KIND_DONE,
            payload: Vec::new(),
        };
        assert!(FileFrame::from_frame(wrong_channel).is_err());
        let wrong_kind = Frame {
            channel: ChannelId::FILE,
            kind: 9,
            payload: Vec::new(),
        };
        assert!(FileFrame::from_frame(wrong_kind).is_err());
    }
}
