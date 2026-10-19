//! 接收线程：拼清单、清洗路径、顶层占名、写 `.part`、校验后改成正式名称。

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use baton_core::files::{Entry, EntryKind, Phase, clean_path};
use baton_core::ids::Platform;
use baton_core::protocol::Message;
use baton_core::{Error, Result};
use baton_net::file::{FileDownload, FileFrame};
use sha2::{Digest, Sha256};

use crate::disk::{self, part_path};
use crate::{Inner, Key, hex};

struct Current {
    index: usize,
    path: PathBuf,
    file: File,
    hasher: Sha256,
    written: u64,
    /// 写入出错或超出声明长度后不再写，End 时判为失败。
    broken: bool,
}

impl Current {
    fn discard(self) {
        drop(self.file);
        let _ = fs::remove_file(part_path(&self.path));
    }
}

struct Receiver<'a> {
    inner: &'a Inner,
    key: &'a Key,
    root: &'a Path,
    entries: Vec<Entry>,
    ready: bool,
    /// 清单首段 → 本机实际的顶层文件夹。
    tops: HashMap<String, PathBuf>,
    current: Option<Current>,
    finished: Vec<bool>,
    done_bytes: u64,
    received: u32,
    failed: u32,
    location: Option<PathBuf>,
}

pub(crate) fn run(inner: &Inner, key: &Key, root: &Path, mut download: FileDownload) {
    let (peer, id) = key;
    let mut rx = Receiver {
        inner,
        key,
        root,
        entries: Vec::new(),
        ready: false,
        tops: HashMap::new(),
        current: None,
        finished: Vec::new(),
        done_bytes: 0,
        received: 0,
        failed: 0,
        location: None,
    };
    let result = rx.receive(&mut download);
    if let Some(current) = rx.current.take() {
        current.discard();
    }
    drop(download);
    match result {
        Ok(true) => {
            let (received, failed) = (rx.received, rx.failed);
            if let Some(location) = &rx.location {
                lock_set_location(inner, key, location);
            }
            if !inner.end(peer, *id, Phase::Done { received, failed }) {
                return;
            }
            inner.send_message(
                peer,
                Message::FileResult {
                    session_id: *id,
                    received,
                    failed,
                },
            );
            match &inner.observer {
                Some(observer) => observer.received(peer, received, rx.location.as_deref()),
                None => inner
                    .notifier
                    .notify(&format!("已收到来自 {} 的 {received} 个文件", peer.0)),
            }
            if failed > 0 {
                inner
                    .notifier
                    .notify(&format!("来自 {} 的文件有 {failed} 个未能收下", peer.0));
            }
        }
        Ok(false) => {}
        Err(e) if inner.is_canceled(key) => log::info!("来自 {} 的传输已结束：{e}", peer.0),
        Err(e) => {
            inner.send_message(peer, Message::FileCancel { session_id: *id });
            inner.fail(
                key,
                &format!("未能接收来自 {} 的文件：{}", peer.0, reason(&e)),
            );
        }
    }
}

fn lock_set_location(inner: &Inner, key: &Key, location: &Path) {
    crate::lock(&inner.sessions).set_location(&key.0, key.1, location.display().to_string());
}

fn reason(e: &Error) -> String {
    match e {
        Error::File(m) | Error::Net(m) => m.clone(),
        other => other.to_string(),
    }
}

fn io(path: &Path, e: std::io::Error) -> Error {
    Error::File(format!("写入 {} 失败：{e}", path.display()))
}

impl Receiver<'_> {
    /// 收到 Done 返回 `Ok(true)`；被取消返回 `Ok(false)`。
    fn receive(&mut self, download: &mut FileDownload) -> Result<bool> {
        loop {
            if self.inner.is_canceled(self.key) {
                return Ok(false);
            }
            let frame = match download.recv() {
                Some(frame) => frame?,
                None => return Err(Error::File("文件连接已断开".into())),
            };
            match frame {
                FileFrame::Manifest(part) if !self.ready => self.entries.extend(part),
                FileFrame::Manifest(_) => return Err(Error::File("清单出现在文件之后".into())),
                FileFrame::Begin(index) => self.begin(index)?,
                FileFrame::Chunk(data) => self.chunk(&data)?,
                FileFrame::End { sha256 } => self.end(sha256.as_deref())?,
                FileFrame::Done => {
                    self.prepare()?;
                    if let Some(current) = self.current.take() {
                        current.discard();
                    }
                    let missing = self
                        .entries
                        .iter()
                        .zip(&self.finished)
                        .filter(|(e, done)| e.kind == EntryKind::File && !**done)
                        .count();
                    self.failed += missing as u32;
                    return Ok(true);
                }
            }
        }
    }

    /// 清单收齐：清洗全部路径，建好文件夹。
    fn prepare(&mut self) -> Result<()> {
        if self.ready {
            return Ok(());
        }
        self.ready = true;
        let target = Platform::current();
        for entry in &mut self.entries {
            entry.path = clean_path(&entry.path, target)?;
        }
        self.finished = vec![false; self.entries.len()];
        for index in 0..self.entries.len() {
            if self.entries[index].kind == EntryKind::Dir {
                let dir = self.resolve_dir(index)?;
                fs::create_dir_all(&dir).map_err(|e| io(&dir, e))?;
                self.finished[index] = true;
            }
        }
        Ok(())
    }

    /// 文件夹条目在本机的路径；顶层文件夹在第一次出现时占名。
    fn resolve_dir(&mut self, index: usize) -> Result<PathBuf> {
        let path = self.entries[index].path.clone();
        let top = self.top_dir(&path[0])?;
        Ok(path[1..].iter().fold(top, |dir, s| dir.join(s)))
    }

    fn top_dir(&mut self, name: &str) -> Result<PathBuf> {
        if let Some(dir) = self.tops.get(name) {
            return Ok(dir.clone());
        }
        let dir = disk::claim_dir(self.root, name).map_err(|e| io(self.root, e))?;
        self.location.get_or_insert_with(|| dir.clone());
        self.tops.insert(name.to_owned(), dir.clone());
        Ok(dir)
    }

    fn begin(&mut self, index: u64) -> Result<()> {
        self.prepare()?;
        if let Some(previous) = self.current.take() {
            previous.discard();
        }
        let index = usize::try_from(index).unwrap_or(usize::MAX);
        let path = match self.entries.get(index) {
            Some(Entry {
                kind: EntryKind::File,
                path,
                ..
            }) => path.clone(),
            _ => return Err(Error::File("文件序号不在清单里".into())),
        };
        let (path, file) = if let [name] = path.as_slice() {
            let (path, file) = disk::claim_file(self.root, name).map_err(|e| io(self.root, e))?;
            self.location.get_or_insert_with(|| path.clone());
            (path, file)
        } else {
            let top = self.top_dir(&path[0])?;
            let parent = path[1..path.len() - 1]
                .iter()
                .fold(top, |dir, s| dir.join(s));
            fs::create_dir_all(&parent).map_err(|e| io(&parent, e))?;
            let name = &path[path.len() - 1];
            disk::claim_file(&parent, name).map_err(|e| io(&parent, e))?
        };
        self.current = Some(Current {
            index,
            path,
            file,
            hasher: Sha256::new(),
            written: 0,
            broken: false,
        });
        Ok(())
    }

    fn chunk(&mut self, data: &[u8]) -> Result<()> {
        let Some(current) = &mut self.current else {
            return Err(Error::File("数据出现在文件开始之前".into()));
        };
        let size = self.entries[current.index].size;
        if current.broken {
            return Ok(());
        }
        if current.written + data.len() as u64 > size {
            current.broken = true;
            return Ok(());
        }
        if let Err(e) = current.file.write_all(data) {
            log::warn!("写入 {} 失败：{e}", current.path.display());
            current.broken = true;
            return Ok(());
        }
        current.hasher.update(data);
        current.written += data.len() as u64;
        self.done_bytes += data.len() as u64;
        self.inner.progress(self.key, self.done_bytes);
        Ok(())
    }

    fn end(&mut self, sha256: Option<&str>) -> Result<()> {
        let Some(current) = self.current.take() else {
            return Err(Error::File("文件结束标记没有对应的文件".into()));
        };
        let size = self.entries[current.index].size;
        let index = current.index;
        let Current {
            path,
            mut file,
            hasher,
            written,
            broken,
            ..
        } = current;
        let digest = hex(&hasher.finalize());
        let ok = !broken
            && written == size
            && sha256 == Some(digest.as_str())
            && file.flush().and_then(|()| file.sync_all()).is_ok();
        drop(file);
        let part = part_path(&path);
        if ok && fs::rename(&part, &path).is_ok() {
            self.received += 1;
        } else {
            let _ = fs::remove_file(&part);
            self.failed += 1;
            log::info!("{} 未通过校验，已丢弃", path.display());
        }
        self.finished[index] = true;
        Ok(())
    }
}
