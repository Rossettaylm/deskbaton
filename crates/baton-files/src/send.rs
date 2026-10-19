//! 上传线程：按清单顺序读文件、边读边算 SHA-256、写进文件连接。

use std::fs::File;
use std::io::{ErrorKind, Read};

use baton_core::Result;
use baton_core::files::EntryKind;
use baton_core::protocol::Message;
use baton_net::file::{FILE_CHUNK_LEN, FileFrame, FileUpload};
use sha2::{Digest, Sha256};

use crate::walk::Source;
use crate::{Inner, Key, hex};

pub(crate) fn upload(inner: &Inner, key: &Key, token: &str, sources: &[Source]) {
    let (peer, id) = key;
    let result = (|| -> Result<bool> {
        let upload = inner.link.open_upload(peer, *id, token)?;
        upload.send(FileFrame::Manifest(
            sources.iter().map(|s| s.entry.clone()).collect(),
        ))?;
        let mut done = 0u64;
        for (index, source) in sources.iter().enumerate() {
            if inner.is_canceled(key) {
                return Ok(false);
            }
            let (EntryKind::File, Some(path)) = (source.entry.kind, &source.path) else {
                continue;
            };
            upload.send(FileFrame::Begin(index as u64))?;
            let sha256 = match File::open(path) {
                Ok(file) => send_file(inner, key, &upload, file, source.entry.size, &mut done)?,
                Err(e) => {
                    log::info!("读不到 {}：{e}", path.display());
                    None
                }
            };
            if inner.is_canceled(key) {
                return Ok(false);
            }
            upload.send(FileFrame::End { sha256 })?;
        }
        upload.send(FileFrame::Done)?;
        upload.finish()?;
        Ok(true)
    })();
    match result {
        Ok(_) => {}
        Err(e) if inner.is_canceled(key) => log::info!("发给 {} 的传输已结束：{e}", peer.0),
        Err(e) => {
            inner.send_message(peer, Message::FileCancel { session_id: *id });
            inner.fail(key, &format!("发给 {} 的文件传输中断：{e}", peer.0));
        }
    }
}

/// 只发清单里声明的长度；文件变短、变长或读失败时返回 `None`，接收方丢弃它。
fn send_file(
    inner: &Inner,
    key: &Key,
    upload: &FileUpload,
    mut file: File,
    size: u64,
    done: &mut u64,
) -> Result<Option<String>> {
    let mut hasher = Sha256::new();
    let mut left = size;
    let mut buf = vec![0u8; FILE_CHUNK_LEN];
    while left > 0 {
        if inner.is_canceled(key) {
            return Ok(None);
        }
        let want = buf.len().min(usize::try_from(left).unwrap_or(usize::MAX));
        let n = match file.read(&mut buf[..want]) {
            Ok(0) => return Ok(None),
            Ok(n) => n,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => {
                log::info!("读取文件失败：{e}");
                return Ok(None);
            }
        };
        hasher.update(&buf[..n]);
        upload.send(FileFrame::Chunk(buf[..n].to_vec()))?;
        left -= n as u64;
        *done += n as u64;
        inner.progress(key, *done);
    }
    let grew = !matches!(file.read(&mut [0u8; 1]), Ok(0));
    Ok((!grew).then(|| hex(&hasher.finalize())))
}
