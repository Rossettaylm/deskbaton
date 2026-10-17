//! 设备档案 `peers.toml`：已配对设备的指纹，以及在线期间记下的系统、输入源编码、WoL MAC 与地址。

use std::fs;
use std::io::{ErrorKind, Write};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use baton_core::event::PeerInfo;
use baton_core::ids::{DeviceId, InputCode, MacAddr, Platform};
use baton_core::{Error, Result};
use serde::{Deserialize, Serialize};

use crate::auth::{self, Fingerprint};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairedPeer {
    pub name: String,
    /// 证书 SHA-256 指纹，十六进制。
    pub fingerprint: String,
    /// 配对时间，Unix 秒。
    #[serde(default)]
    pub paired_at: u64,
    #[serde(default)]
    pub platform: Option<Platform>,
    #[serde(default)]
    pub input_code: Option<InputCode>,
    #[serde(default)]
    pub wol_mac: Option<MacAddr>,
    #[serde(default)]
    pub last_ip: Option<IpAddr>,
    /// 对端监听端口。
    #[serde(default)]
    pub port: Option<u16>,
}

impl PairedPeer {
    pub fn new(name: &str, fingerprint: &Fingerprint) -> Self {
        Self {
            name: name.to_owned(),
            fingerprint: auth::to_hex(fingerprint),
            paired_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            platform: None,
            input_code: None,
            wol_mac: None,
            last_ip: None,
            port: None,
        }
    }

    pub fn device_id(&self) -> DeviceId {
        DeviceId(self.name.clone())
    }

    /// 交给协调器的「已知设备」信息，对端不在线时也能据此唤醒。
    pub fn info(&self) -> PeerInfo {
        PeerInfo {
            device_id: self.device_id(),
            platform: self.platform,
            input_code: self.input_code,
            wol_mac: self.wol_mac,
            ip: self.last_ip,
        }
    }

    pub fn matches(&self, fingerprint: &Fingerprint) -> bool {
        auth::parse_fingerprint(&self.fingerprint).is_some_and(|fp| fp == *fingerprint)
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)]
    peers: Vec<PairedPeer>,
}

/// 进程内共享一个实例；所有修改经同一把锁，并立即写回文件。
pub struct PeerStore {
    path: Option<PathBuf>,
    peers: Mutex<Vec<PairedPeer>>,
}

impl PeerStore {
    /// 文件不存在时为空档案。
    pub fn open(path: &Path) -> Result<Self> {
        let peers = match fs::read_to_string(path) {
            Ok(text) => {
                toml::from_str::<File>(&text)
                    .map_err(|e| Error::Config(format!("设备档案 {} 有误：{e}", path.display())))?
                    .peers
            }
            Err(e) if e.kind() == ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(io_error(path, e)),
        };
        Ok(Self {
            path: Some(path.to_owned()),
            peers: Mutex::new(peers),
        })
    }

    /// 不落盘的档案，用于测试。
    pub fn in_memory() -> Self {
        Self {
            path: None,
            peers: Mutex::new(Vec::new()),
        }
    }

    pub fn list(&self) -> Vec<PairedPeer> {
        self.lock().clone()
    }

    pub fn find(&self, fingerprint: &Fingerprint) -> Option<PairedPeer> {
        self.lock().iter().find(|p| p.matches(fingerprint)).cloned()
    }

    /// 写入一台设备：同指纹或同名称的旧条目被替换（对端重装后重新配对）。
    pub fn upsert(&self, peer: PairedPeer) -> Result<()> {
        let mut peers = self.lock();
        peers.retain(|p| p.fingerprint != peer.fingerprint && p.name != peer.name);
        peers.push(peer);
        peers.sort_by(|a, b| a.name.cmp(&b.name));
        self.save(&peers)
    }

    /// 握手成功后刷新；`wol_mac` 为空时保留旧值。设备不在档案里时什么也不做。
    pub fn refresh(
        &self,
        fingerprint: &Fingerprint,
        info: &PeerInfo,
        port: Option<u16>,
    ) -> Result<()> {
        let mut peers = self.lock();
        let Some(index) = peers.iter().position(|p| p.matches(fingerprint)) else {
            return Ok(());
        };
        let mut updated = peers[index].clone();
        updated.name.clone_from(&info.device_id.0);
        updated.platform = info.platform.or(updated.platform);
        updated.input_code = info.input_code.or(updated.input_code);
        updated.wol_mac = info.wol_mac.or(updated.wol_mac);
        updated.last_ip = info.ip.or(updated.last_ip);
        updated.port = port.or(updated.port);
        if updated == peers[index] {
            return Ok(());
        }
        let renamed = updated.name != peers[index].name;
        peers[index] = updated.clone();
        if renamed {
            peers.retain(|p| p.name != updated.name || p.fingerprint == updated.fingerprint);
        }
        self.save(&peers)
    }

    pub fn remove(&self, fingerprint: &Fingerprint) -> Result<Option<PairedPeer>> {
        let mut peers = self.lock();
        let Some(index) = peers.iter().position(|p| p.matches(fingerprint)) else {
            return Ok(None);
        };
        let removed = peers.remove(index);
        self.save(&peers)?;
        Ok(Some(removed))
    }

    fn lock(&self) -> MutexGuard<'_, Vec<PairedPeer>> {
        self.peers.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// 先写临时文件再重命名，崩溃时不会留下写了一半的档案。
    fn save(&self, peers: &[PairedPeer]) -> Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let text = toml::to_string(&File {
            peers: peers.to_vec(),
        })
        .map_err(|e| Error::Config(format!("无法序列化设备档案：{e}")))?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| io_error(dir, e))?;
        }
        let tmp = path.with_extension("toml.tmp");
        let mut file = fs::File::create(&tmp).map_err(|e| io_error(&tmp, e))?;
        file.write_all(text.as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|e| io_error(&tmp, e))?;
        fs::rename(&tmp, path).map_err(|e| io_error(path, e))
    }
}

fn io_error(path: &Path, e: std::io::Error) -> Error {
    Error::Config(format!("读写 {} 失败：{e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: Fingerprint = [1; 32];
    const B: Fingerprint = [2; 32];

    fn info(name: &str) -> PeerInfo {
        PeerInfo {
            device_id: DeviceId(name.into()),
            platform: Some(Platform::Windows),
            input_code: Some(InputCode(0x0F)),
            wol_mac: Some("aa:bb:cc:dd:ee:ff".parse().unwrap()),
            ip: Some("192.168.1.20".parse().unwrap()),
        }
    }

    #[test]
    fn roundtrips_through_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("peers.toml");
        let store = PeerStore::open(&path).unwrap();
        assert!(store.list().is_empty());
        store.upsert(PairedPeer::new("win", &A)).unwrap();
        store.refresh(&A, &info("win"), Some(47800)).unwrap();

        let reopened = PeerStore::open(&path).unwrap();
        let peer = reopened.find(&A).unwrap();
        assert_eq!(peer.name, "win");
        assert_eq!(peer.platform, Some(Platform::Windows));
        assert_eq!(peer.input_code, Some(InputCode(0x0F)));
        assert_eq!(peer.last_ip, Some("192.168.1.20".parse().unwrap()));
        assert_eq!(peer.port, Some(47800));
        assert_eq!(peer.info(), info("win"));
    }

    #[test]
    fn upsert_replaces_same_name_or_fingerprint() {
        let store = PeerStore::in_memory();
        store.upsert(PairedPeer::new("win", &A)).unwrap();
        store.upsert(PairedPeer::new("win", &B)).unwrap();
        assert_eq!(store.list().len(), 1);
        assert!(store.find(&A).is_none());
        store.upsert(PairedPeer::new("desktop", &B)).unwrap();
        assert_eq!(store.list().len(), 1);
        assert_eq!(store.find(&B).unwrap().name, "desktop");
    }

    #[test]
    fn refresh_keeps_known_mac_and_ignores_strangers() {
        let store = PeerStore::in_memory();
        store.upsert(PairedPeer::new("win", &A)).unwrap();
        store.refresh(&A, &info("win"), None).unwrap();
        let mut without_mac = info("win-renamed");
        without_mac.wol_mac = None;
        store.refresh(&A, &without_mac, None).unwrap();
        let peer = store.find(&A).unwrap();
        assert_eq!(peer.name, "win-renamed");
        assert_eq!(peer.wol_mac, info("win").wol_mac);

        store.refresh(&B, &info("stranger"), None).unwrap();
        assert_eq!(store.list().len(), 1);
    }

    #[test]
    fn remove_deletes_the_entry() {
        let store = PeerStore::in_memory();
        store.upsert(PairedPeer::new("win", &A)).unwrap();
        assert_eq!(store.remove(&A).unwrap().unwrap().name, "win");
        assert!(store.remove(&A).unwrap().is_none());
    }

    #[test]
    fn rejects_unknown_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("peers.toml");
        fs::write(
            &path,
            "[[peers]]\nname = \"a\"\nfingerprint = \"00\"\ncolor = 1\n",
        )
        .unwrap();
        assert!(PeerStore::open(&path).is_err());
    }
}
