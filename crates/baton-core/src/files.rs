//! 文件快传的纯逻辑：清单与请求摘要、路径清洗、同名改名、空间判定、会话表。
//! 设计见 `docs/superpowers/specs/2026-10-19-file-transfer-design.md`。

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::ids::{DeviceId, Platform};
use crate::{Error, Result};

pub type SessionId = u64;

/// 目标离线时，唤醒后等它上线的时长。
pub const WAKE_WAIT: Duration = Duration::from_secs(30);
/// 请求摘要里最多列出的顶层条目名。
pub const SUMMARY_NAMES: usize = 5;
/// 结束的会话保留多久；自动接收的「刚收到」提示也只在这段时间内弹出。
pub const RECENT: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    File,
    Dir,
}

/// 清单中的一项。`path` 按段给出，顶层条目只有一段；目录先于其中的内容出现。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub path: Vec<String>,
    pub kind: EntryKind,
    #[serde(default)]
    pub size: u64,
}

/// `file_offer` 携带的摘要；完整清单在同意后从文件连接下发。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OfferSummary {
    pub files: u32,
    pub dirs: u32,
    pub total_bytes: u64,
    pub names: Vec<String>,
}

impl OfferSummary {
    pub fn of(entries: &[Entry]) -> Self {
        let mut summary = Self::default();
        for entry in entries {
            match entry.kind {
                EntryKind::File => {
                    summary.files += 1;
                    summary.total_bytes += entry.size;
                }
                EntryKind::Dir => summary.dirs += 1,
            }
            if let [name] = entry.path.as_slice()
                && summary.names.len() < SUMMARY_NAMES
            {
                summary.names.push(name.clone());
            }
        }
        summary
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectReason {
    Declined,
    NoSpace,
    /// 无界面且需要确认。
    Unavailable,
    #[serde(other)]
    Error,
}

impl RejectReason {
    pub fn describe(self, peer: &DeviceId) -> String {
        match self {
            Self::Declined => format!("{} 拒绝了接收", peer.0),
            Self::NoSpace => format!("{} 的磁盘空间不足", peer.0),
            Self::Unavailable => format!("{} 没有打开面板，无法确认接收", peer.0),
            Self::Error => format!("{} 无法接收", peer.0),
        }
    }
}

/// 让清单中的一段在 `target` 上成为合法文件名。`/`、NUL、空段、`.`、`..` 视为恶意路径。
pub fn clean_segment(segment: &str, target: Platform) -> Result<String> {
    if segment.is_empty() || segment == "." || segment == ".." || segment.contains(['/', '\0']) {
        return Err(Error::File(format!("文件路径不合法：{segment:?}")));
    }
    let mut name: String = segment.chars().filter(|c| !c.is_control()).collect();
    if target == Platform::Windows {
        name = name.chars().map(fullwidth).collect();
        name.truncate(name.trim_end_matches([' ', '.']).len());
        let stem = name.split('.').next().unwrap_or_default();
        if is_reserved(stem) {
            name.insert(stem.len(), '_');
        }
    }
    if name.is_empty() {
        name.push('_');
    }
    Ok(name)
}

pub fn clean_path(path: &[String], target: Platform) -> Result<Vec<String>> {
    if path.is_empty() {
        return Err(Error::File("文件路径为空".into()));
    }
    path.iter().map(|s| clean_segment(s, target)).collect()
}

fn fullwidth(c: char) -> char {
    match c {
        '<' => '＜',
        '>' => '＞',
        ':' => '：',
        '"' => '＂',
        '\\' => '＼',
        '|' => '｜',
        '?' => '？',
        '*' => '＊',
        other => other,
    }
}

fn is_reserved(stem: &str) -> bool {
    let upper = stem.to_ascii_uppercase();
    matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|prefix| {
            upper
                .strip_prefix(prefix)
                .is_some_and(|n| n.len() == 1 && matches!(n.as_bytes()[0], b'1'..=b'9'))
        })
}

/// 同名时的第 `n` 个候选：`报告 (1).pdf`、`照片 (1)`。开头的点不算扩展名分隔。
pub fn numbered(name: &str, n: u32, kind: EntryKind) -> String {
    if n == 0 {
        return name.to_owned();
    }
    let split = match kind {
        EntryKind::File => name.rfind('.').filter(|&i| i > 0),
        EntryKind::Dir => None,
    };
    match split {
        Some(i) => format!("{} ({n}){}", &name[..i], &name[i..]),
        None => format!("{name} ({n})"),
    }
}

/// 剩余空间未知时不拦。
pub fn has_room(free: Option<u64>, need: u64, reserved: u64) -> bool {
    free.is_none_or(|free| free >= need.saturating_add(reserved))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Send,
    Receive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum Phase {
    /// 发送方：目标离线，已发唤醒信号，等它上线。
    Waking,
    /// 发送方：已发请求，等对方同意。
    Offered,
    /// 接收方：等本机用户同意。
    Pending,
    Transferring,
    Done {
        received: u32,
        failed: u32,
    },
    Failed {
        reason: String,
    },
    Rejected {
        reason: RejectReason,
    },
    Canceled {
        by_peer: bool,
    },
}

impl Phase {
    pub fn is_active(&self) -> bool {
        matches!(
            self,
            Self::Waking | Self::Offered | Self::Pending | Self::Transferring
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionView {
    pub id: SessionId,
    pub peer: DeviceId,
    pub direction: Direction,
    #[serde(flatten)]
    pub phase: Phase,
    pub summary: OfferSummary,
    pub done_bytes: u64,
    /// 接收方：收下的第一个顶层条目的完整路径，供「在访达中显示」。
    pub location: Option<String>,
    /// 接收方的结果已在面板上看过。
    pub seen: bool,
}

struct Session {
    view: SessionView,
    seq: u64,
    token: Option<String>,
    wake_deadline: Option<Instant>,
    ended_at: Option<Instant>,
}

type Key = (DeviceId, SessionId);

/// 两个方向的全部会话。会话按 `(对端, id)` 区分，id 由发送方生成。
#[derive(Default)]
pub struct Sessions {
    map: HashMap<Key, Session>,
    next_seq: u64,
}

impl Sessions {
    fn insert(
        &mut self,
        peer: &DeviceId,
        view: SessionView,
        wake_deadline: Option<Instant>,
    ) -> bool {
        let key = (peer.clone(), view.id);
        if self.map.contains_key(&key) {
            return false;
        }
        self.next_seq += 1;
        self.map.insert(
            key,
            Session {
                view,
                seq: self.next_seq,
                token: None,
                wake_deadline,
                ended_at: None,
            },
        );
        true
    }

    fn get_mut(&mut self, peer: &DeviceId, id: SessionId) -> Option<&mut Session> {
        self.map.get_mut(&(peer.clone(), id))
    }

    /// 本机发起：目标在线直接进入 `Offered`，否则 `Waking`，最多等 [`WAKE_WAIT`]。
    pub fn send(
        &mut self,
        peer: &DeviceId,
        id: SessionId,
        summary: OfferSummary,
        online: bool,
        now: Instant,
    ) -> bool {
        let phase = if online {
            Phase::Offered
        } else {
            Phase::Waking
        };
        let deadline = (!online).then(|| now + WAKE_WAIT);
        self.insert(
            peer,
            view(peer, id, Direction::Send, phase, summary),
            deadline,
        )
    }

    /// 对端发来请求，先挂起；自动接收时调用方紧接着 [`Self::start`]。
    pub fn receive(&mut self, peer: &DeviceId, id: SessionId, summary: OfferSummary) -> bool {
        let view = view(peer, id, Direction::Receive, Phase::Pending, summary);
        self.insert(peer, view, None)
    }

    /// 目标上线：等待中的会话转为 `Offered`，返回这些会话，调用方据此发出请求。
    pub fn peer_online(&mut self, peer: &DeviceId) -> Vec<SessionId> {
        let mut ids = Vec::new();
        for ((p, id), s) in &mut self.map {
            if p == peer && s.view.phase == Phase::Waking {
                s.view.phase = Phase::Offered;
                s.wake_deadline = None;
                ids.push(*id);
            }
        }
        ids.sort_unstable();
        ids
    }

    /// 发送方收到同意，或接收方同意（附上发给对方的令牌）后开始传输。
    pub fn start(&mut self, peer: &DeviceId, id: SessionId, token: Option<String>) -> bool {
        let Some(s) = self.get_mut(peer, id) else {
            return false;
        };
        let from = match s.view.direction {
            Direction::Send => Phase::Offered,
            Direction::Receive => Phase::Pending,
        };
        if s.view.phase != from {
            return false;
        }
        s.view.phase = Phase::Transferring;
        s.token = token;
        true
    }

    /// 文件连接出示的令牌：只能用一次。
    pub fn claim_token(&mut self, peer: &DeviceId, id: SessionId, token: &str) -> bool {
        let Some(s) = self.get_mut(peer, id) else {
            return false;
        };
        let ok = s.view.direction == Direction::Receive
            && s.view.phase == Phase::Transferring
            && s.token.as_deref() == Some(token);
        if ok {
            s.token = None;
        }
        ok
    }

    pub fn progress(&mut self, peer: &DeviceId, id: SessionId, done_bytes: u64) {
        if let Some(s) = self.get_mut(peer, id)
            && s.view.phase == Phase::Transferring
        {
            s.view.done_bytes = done_bytes;
        }
    }

    pub fn set_location(&mut self, peer: &DeviceId, id: SessionId, location: String) {
        if let Some(s) = self.get_mut(peer, id) {
            s.view.location.get_or_insert(location);
        }
    }

    /// 进入终态；已经结束的会话不再改变，返回 `false`。
    pub fn finish(&mut self, peer: &DeviceId, id: SessionId, phase: Phase, now: Instant) -> bool {
        debug_assert!(!phase.is_active());
        let Some(s) = self.get_mut(peer, id) else {
            return false;
        };
        if !s.view.phase.is_active() {
            return false;
        }
        s.view.phase = phase;
        s.wake_deadline = None;
        s.token = None;
        s.ended_at = Some(now);
        true
    }

    /// 与 `peer` 的长连接断开：除等待唤醒的之外全部结束，返回被结束的会话。
    pub fn peer_offline(&mut self, peer: &DeviceId, now: Instant) -> Vec<SessionId> {
        let ids: Vec<SessionId> = self
            .map
            .iter()
            .filter(|((p, _), s)| {
                p == peer && s.view.phase.is_active() && s.view.phase != Phase::Waking
            })
            .map(|((_, id), _)| *id)
            .collect();
        for id in &ids {
            let reason = format!("与 {} 的连接已断开", peer.0);
            self.finish(peer, *id, Phase::Failed { reason }, now);
        }
        ids
    }

    /// 唤醒超时的会话判为失败并返回。结束超过 [`RECENT`] 的会话清掉，失败的留下等面板收起。
    pub fn tick(&mut self, now: Instant) -> Vec<(DeviceId, SessionId)> {
        let expired: Vec<Key> = self
            .map
            .iter()
            .filter(|(_, s)| s.wake_deadline.is_some_and(|d| now >= d))
            .map(|(k, _)| k.clone())
            .collect();
        for (peer, id) in &expired {
            let reason = format!("{} 未能唤醒，未发送", peer.0);
            self.finish(peer, *id, Phase::Failed { reason }, now);
        }
        self.map.retain(|_, s| {
            matches!(s.view.phase, Phase::Failed { .. })
                || s.ended_at.is_none_or(|at| now.duration_since(at) < RECENT)
        });
        expired
    }

    /// 下一次需要 [`Self::tick`] 的时间。
    pub fn next_deadline(&self) -> Option<Instant> {
        self.map.values().filter_map(|s| s.wake_deadline).min()
    }

    /// 接收中尚未写完的字节数，同意新请求时从剩余空间里扣掉。
    pub fn reserved_bytes(&self) -> u64 {
        self.map
            .values()
            .filter(|s| {
                s.view.direction == Direction::Receive && s.view.phase == Phase::Transferring
            })
            .map(|s| s.view.summary.total_bytes.saturating_sub(s.view.done_bytes))
            .sum()
    }

    pub fn get(&self, peer: &DeviceId, id: SessionId) -> Option<&SessionView> {
        self.map.get(&(peer.clone(), id)).map(|s| &s.view)
    }

    /// 按发起先后排列。
    pub fn views(&self) -> Vec<SessionView> {
        let mut sessions: Vec<&Session> = self.map.values().collect();
        sessions.sort_by_key(|s| s.seq);
        sessions.into_iter().map(|s| s.view.clone()).collect()
    }

    /// 从面板上收起已结束的会话。
    pub fn dismiss(&mut self, peer: &DeviceId, id: SessionId) {
        let key = (peer.clone(), id);
        if self
            .map
            .get(&key)
            .is_some_and(|s| !s.view.phase.is_active())
        {
            self.map.remove(&key);
        }
    }

    /// 面板已经展示过接收结果。
    pub fn mark_seen(&mut self) {
        for s in self.map.values_mut() {
            if s.view.direction == Direction::Receive && !s.view.phase.is_active() {
                s.view.seen = true;
            }
        }
    }

    /// 有等本机处理的请求，或 [`RECENT`] 内收到、还没看过的文件：本机接棒时应弹出面板。
    pub fn needs_attention(&self, now: Instant) -> bool {
        self.map.values().any(|s| {
            s.view.direction == Direction::Receive
                && match &s.view.phase {
                    Phase::Pending => true,
                    Phase::Done { received, .. } => {
                        *received > 0
                            && !s.view.seen
                            && s.ended_at.is_some_and(|at| now.duration_since(at) < RECENT)
                    }
                    _ => false,
                }
        })
    }

    /// 某个对端相关的、仍在进行的会话。
    pub fn active_with(&self, peer: &DeviceId) -> Vec<SessionId> {
        self.map
            .iter()
            .filter(|((p, _), s)| p == peer && s.view.phase.is_active())
            .map(|((_, id), _)| *id)
            .collect()
    }
}

fn view(
    peer: &DeviceId,
    id: SessionId,
    direction: Direction,
    phase: Phase,
    summary: OfferSummary,
) -> SessionView {
    SessionView {
        id,
        peer: peer.clone(),
        direction,
        phase,
        summary,
        done_bytes: 0,
        location: None,
        seen: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win() -> DeviceId {
        DeviceId("win".into())
    }

    fn file(path: &[&str], size: u64) -> Entry {
        Entry {
            path: path.iter().map(|s| (*s).to_owned()).collect(),
            kind: EntryKind::File,
            size,
        }
    }

    fn dir(path: &[&str]) -> Entry {
        Entry {
            path: path.iter().map(|s| (*s).to_owned()).collect(),
            kind: EntryKind::Dir,
            size: 0,
        }
    }

    fn summary(total: u64) -> OfferSummary {
        OfferSummary {
            files: 1,
            dirs: 0,
            total_bytes: total,
            names: vec!["a.txt".into()],
        }
    }

    #[test]
    fn summary_counts_and_lists_top_level_names() {
        let mut entries = vec![
            dir(&["照片"]),
            file(&["照片", "1.jpg"], 10),
            dir(&["照片", "空"]),
        ];
        for i in 0..6 {
            entries.push(file(&[&format!("{i}.txt")], 1));
        }
        let s = OfferSummary::of(&entries);
        assert_eq!((s.files, s.dirs, s.total_bytes), (7, 2, 16));
        assert_eq!(s.names, ["照片", "0.txt", "1.txt", "2.txt", "3.txt"]);
    }

    #[test]
    fn windows_names_use_fullwidth_and_drop_trailing_dots() {
        let clean = |s| clean_segment(s, Platform::Windows).unwrap();
        assert_eq!(clean("会议:纪要?.txt"), "会议：纪要？.txt");
        assert_eq!(clean(r#"a<b>c"d\e|f*g"#), "a＜b＞c＂d＼e｜f＊g");
        assert_eq!(clean("报告. . "), "报告");
        assert_eq!(clean("tab\there"), "tabhere");
        assert_eq!(clean("..."), "_");
    }

    #[test]
    fn windows_reserved_names_get_underscore() {
        let clean = |s| clean_segment(s, Platform::Windows).unwrap();
        assert_eq!(clean("CON"), "CON_");
        assert_eq!(clean("nul.txt"), "nul_.txt");
        assert_eq!(clean("com3.tar.gz"), "com3_.tar.gz");
        assert_eq!(clean("COM0"), "COM0");
        assert_eq!(clean("console"), "console");
    }

    #[test]
    fn macos_keeps_windows_illegal_characters() {
        let clean = |s| clean_segment(s, Platform::Macos).unwrap();
        assert_eq!(clean("会议:纪要?.txt"), "会议:纪要?.txt");
        assert_eq!(clean("CON"), "CON");
        assert_eq!(clean("a\u{7}b"), "ab");
    }

    #[test]
    fn traversal_and_separators_are_rejected() {
        for bad in ["", ".", "..", "a/b", "a\0b"] {
            assert!(clean_segment(bad, Platform::Macos).is_err(), "{bad:?}");
            assert!(clean_segment(bad, Platform::Windows).is_err(), "{bad:?}");
        }
        assert!(clean_path(&[], Platform::Macos).is_err());
        assert!(clean_path(&["ok".into(), "..".into()], Platform::Macos).is_err());
    }

    #[test]
    fn numbered_names_keep_extension() {
        assert_eq!(numbered("报告.pdf", 0, EntryKind::File), "报告.pdf");
        assert_eq!(numbered("报告.pdf", 1, EntryKind::File), "报告 (1).pdf");
        assert_eq!(numbered(".bashrc", 2, EntryKind::File), ".bashrc (2)");
        assert_eq!(numbered("v1.2", 1, EntryKind::Dir), "v1.2 (1)");
        assert_eq!(numbered("README", 3, EntryKind::File), "README (3)");
    }

    #[test]
    fn room_check_subtracts_reserved_and_tolerates_unknown() {
        assert!(has_room(Some(100), 60, 40));
        assert!(!has_room(Some(100), 61, 40));
        assert!(has_room(None, u64::MAX, 1));
    }

    #[test]
    fn offline_target_waits_then_offers_on_online() {
        let now = Instant::now();
        let mut s = Sessions::default();
        assert!(s.send(&win(), 1, summary(5), false, now));
        assert_eq!(s.get(&win(), 1).unwrap().phase, Phase::Waking);
        assert_eq!(s.next_deadline(), Some(now + WAKE_WAIT));
        assert!(s.peer_offline(&win(), now).is_empty(), "等唤醒时离线是常态");
        assert_eq!(s.peer_online(&win()), [1]);
        assert_eq!(s.get(&win(), 1).unwrap().phase, Phase::Offered);
        assert_eq!(s.next_deadline(), None);
    }

    #[test]
    fn wake_timeout_fails_the_send() {
        let now = Instant::now();
        let mut s = Sessions::default();
        s.send(&win(), 1, summary(5), false, now);
        assert!(s.tick(now + Duration::from_secs(29)).is_empty());
        assert_eq!(s.tick(now + WAKE_WAIT), [(win(), 1)]);
        assert!(matches!(
            &s.get(&win(), 1).unwrap().phase,
            Phase::Failed { reason } if reason.contains("未能唤醒")
        ));
    }

    #[test]
    fn send_session_goes_through_accept_and_result() {
        let now = Instant::now();
        let mut s = Sessions::default();
        s.send(&win(), 7, summary(5), true, now);
        assert!(!s.start(&win(), 8, None), "未知会话");
        assert!(s.start(&win(), 7, None));
        assert!(!s.start(&win(), 7, None), "不能重复开始");
        s.progress(&win(), 7, 3);
        assert_eq!(s.get(&win(), 7).unwrap().done_bytes, 3);
        let done = Phase::Done {
            received: 1,
            failed: 0,
        };
        assert!(s.finish(&win(), 7, done.clone(), now));
        assert!(!s.finish(&win(), 7, Phase::Canceled { by_peer: true }, now));
        assert_eq!(s.get(&win(), 7).unwrap().phase, done);
    }

    #[test]
    fn pending_receive_needs_attention_until_decided() {
        let now = Instant::now();
        let mut s = Sessions::default();
        assert!(s.receive(&win(), 3, summary(5)));
        assert!(!s.receive(&win(), 3, summary(5)), "同一会话不重复登记");
        assert!(s.needs_attention(now));
        assert!(!s.claim_token(&win(), 3, "tok"), "同意前没有令牌");
        assert!(s.start(&win(), 3, Some("tok".into())));
        assert!(!s.start(&win(), 3, Some("other".into())), "令牌只发一次");
        assert!(!s.needs_attention(now));
        assert!(!s.claim_token(&win(), 3, "bad"));
        assert!(s.claim_token(&win(), 3, "tok"));
        assert!(!s.claim_token(&win(), 3, "tok"), "令牌只能用一次");
    }

    #[test]
    fn received_files_need_attention_until_seen_or_stale() {
        let now = Instant::now();
        let mut s = Sessions::default();
        s.receive(&win(), 5, summary(5));
        s.start(&win(), 5, None);
        let done = Phase::Done {
            received: 2,
            failed: 0,
        };
        s.finish(&win(), 5, done, now);
        assert!(s.needs_attention(now));
        assert!(!s.needs_attention(now + RECENT));
        s.mark_seen();
        assert!(!s.needs_attention(now));
    }

    #[test]
    fn peer_offline_ends_pending_offered_and_transferring() {
        let now = Instant::now();
        let mut s = Sessions::default();
        s.send(&win(), 1, summary(5), true, now);
        s.receive(&win(), 2, summary(5));
        s.receive(&win(), 3, summary(5));
        s.start(&win(), 3, None);
        s.receive(&DeviceId("mac".into()), 4, summary(5));
        let mut ended = s.peer_offline(&win(), now);
        ended.sort_unstable();
        assert_eq!(ended, [1, 2, 3]);
        assert!(s.get(&DeviceId("mac".into()), 4).unwrap().phase.is_active());
        assert!(s.active_with(&win()).is_empty());
    }

    #[test]
    fn reserved_bytes_count_unwritten_receives() {
        let now = Instant::now();
        let mut s = Sessions::default();
        s.receive(&win(), 1, summary(100));
        s.start(&win(), 1, None);
        s.progress(&win(), 1, 30);
        s.receive(&win(), 2, summary(50));
        s.send(&win(), 3, summary(1000), true, now);
        assert_eq!(s.reserved_bytes(), 70);
    }

    #[test]
    fn failed_sessions_stay_until_dismissed() {
        let now = Instant::now();
        let mut s = Sessions::default();
        s.send(&win(), 1, summary(1), false, now);
        assert_eq!(s.tick(now + WAKE_WAIT), [(win(), 1)]);
        s.tick(now + WAKE_WAIT + RECENT);
        assert!(matches!(
            s.get(&win(), 1).map(|v| &v.phase),
            Some(Phase::Failed { .. })
        ));
        s.dismiss(&win(), 1);
        assert!(s.get(&win(), 1).is_none());
    }

    #[test]
    fn ended_sessions_are_pruned_and_dismissable() {
        let now = Instant::now();
        let mut s = Sessions::default();
        s.send(&win(), 1, summary(1), true, now);
        s.send(&win(), 2, summary(1), true, now);
        s.dismiss(&win(), 1);
        assert_eq!(s.views().len(), 2, "进行中的不能收起");
        s.finish(&win(), 1, Phase::Canceled { by_peer: false }, now);
        s.dismiss(&win(), 1);
        assert_eq!(s.views().iter().map(|v| v.id).collect::<Vec<_>>(), [2]);
        s.finish(&win(), 2, Phase::Canceled { by_peer: false }, now);
        s.tick(now + RECENT);
        assert!(s.views().is_empty());
    }

    #[test]
    fn view_serializes_phase_inline() {
        let v = view(&win(), 9, Direction::Receive, Phase::Pending, summary(1));
        let json = serde_json::to_value(&v).unwrap();
        assert_eq!(json["phase"], "pending");
        assert_eq!(json["direction"], "receive");
        let rejected = serde_json::to_value(Phase::Rejected {
            reason: RejectReason::NoSpace,
        })
        .unwrap();
        assert_eq!(rejected["reason"], "no_space");
    }
}
