// @ts-check

/**
 * @typedef {{ width: number, height: number, pixel_width: number, pixel_height: number,
 *             refresh_hz: number | null, hidpi: boolean, scale_percent: number | null }} ScreenView
 * @typedef {"macos" | "windows" | "linux" | "other"} Platform
 * @typedef {{ name: string, platform: Platform | null, local: boolean }} PortOwner
 * @typedef {{ label: string, code: number, kind: "dp" | "hdmi" | "usb_c" | "other",
 *             active: boolean, owners: PortOwner[] }} InputButton
 * @typedef {{ name: string, platform: Platform | null, local: boolean, online: boolean,
 *             port: string | null, active: boolean }} ComputerView
 * @typedef {{ id: string, name: string, serial: string | null, subtitle: string,
 *             screen: ScreenView | null, current_input: number | null,
 *             current_label: string | null, inputs: InputButton[], computers: ComputerView[] | null,
 *             preferred: boolean }} DisplayView
 * @typedef {{ displays: DisplayView[], config_error: string | null }} Snapshot
 * @typedef {{ kind: "confirmed", code: number, label: string } | { kind: "left_local", label: string }} SwitchOutcome
 * @typedef {{ name: string | null, hotkey: string | null, error: string | null,
 *             input_code_configured: boolean }} AgentStatus
 * @typedef {{ name: string, fingerprint: string, paired_at: number, platform: Platform | null,
 *             input_code: number | null,
 *             wol_mac: string | null, last_ip: string | null, port: number | null }} PairedPeer
 * @typedef {{ id: number, peer: string, fingerprint: string, platform: Platform | null, ip: string | null,
 *             awaiting_me: boolean }} PairingSession
 * @typedef {{ name: string, fingerprint: string | null, address: string }} Candidate
 * @typedef {{ me: string, fingerprint: string, paired: { peer: PairedPeer, online: boolean, holding: boolean }[],
 *             sessions: PairingSession[], candidates: Candidate[] }} Devices
 * @typedef {{ files: number, dirs: number, total_bytes: number, names: string[] }} OfferSummary
 * @typedef {{ id: number, peer: string, direction: "send" | "receive",
 *             phase: "waking" | "offered" | "pending" | "transferring" | "done" | "failed" | "rejected" | "canceled",
 *             received?: number, failed?: number, reason?: string, by_peer?: boolean,
 *             summary: OfferSummary, done_bytes: number, location: string | null, seen: boolean,
 *             can_retry: boolean, reveal: string | null }} FileSession
 * @typedef {{ name: string, platform: Platform | null, online: boolean }} SendTarget
 * @typedef {{ count: number, names: string[], targets: SendTarget[] }} PendingSend
 */

const tauri = /** @type {any} */ (window).__TAURI__;
/** @type {<T>(cmd: string, args?: object) => Promise<T>} */
const invoke = tauri.core.invoke;

const SELECTED_KEY = "deskbaton.display";
const NOTICE_TTL_MS = 3000;

/** @typedef {"handoff" | "devices" | "local"} TabId */

/** @type {TabId[]} */
const TABS = ["handoff", "devices", "local"];

const PORT_PATHS = {
  dp: '<path d="M2 2h26v7l-4 3H2z"/>',
  hdmi: '<path d="M2 2h26v6l-3 4H5l-3-4z"/>',
  usb_c: '<rect x="4" y="2.5" width="22" height="9" rx="4.5"/>',
  other: '<rect x="3" y="2.5" width="24" height="9" rx="1.5"/>',
};

const COMPUTER_PATH = '<rect x="6" y="1.5" width="18" height="9" rx="1.2"/><path d="M3 12.5h24"/>';

/** @type {Record<string, string>} */
const PLATFORM_NAMES = { macos: "macOS", windows: "Windows", linux: "Linux" };

/** @param {PortOwner} o */
const ownerName = (o) => (o.local ? "本机" : o.name);

/** @param {PortOwner[]} owners */
const ownerNames = (owners) => owners.map(ownerName).join("、");

/** @param {PortOwner[]} owners */
const ownerPlatforms = (owners) =>
  [...new Set(owners.map((o) => (o.platform ? PLATFORM_NAMES[o.platform] : undefined)).filter(Boolean))].join(" / ");

const state = {
  /** @type {DisplayView[]} */
  displays: [],
  /** @type {string | null} */
  selectedId: localStorage.getItem(SELECTED_KEY),
  /** @type {string | null} */
  configError: null,
  loading: false,
  /** @type {number | null} 正在切换的目标编码 */
  pending: null,
  /** @type {Map<string, number>} 本机切走后，各显示器被切到的编码 */
  leftTo: new Map(),
  /** @type {{ tone: "idle" | "ok" | "err", text: string, keep?: boolean } | null} 覆盖默认状态行的一次性提示 */
  notice: null,
  /** @type {AgentStatus | null} */
  agent: null,
  /** @type {string | null} */
  version: null,
  /** @type {{ kind: "idle" | "available" | "installing", version: string | null, checking: boolean }} */
  update: { kind: "idle", version: null, checking: false },
  /** @type {Devices | null} */
  devices: null,
  /** @type {string | null} 读取设备失败的原因 */
  devicesError: null,
  /** @type {Set<string>} 正在发起配对的地址 */
  pairing: new Set(),
  /** @type {string | null} 点过一次「移除」、等待再次确认的设备指纹 */
  removing: null,
  /** @type {string | null} 正在交棒的目标电脑 */
  handingTo: null,
  /** @type {TabId} */
  tab: "handoff",
  /** @type {Set<number>} 已经提示过的配对请求，避免用户切回交棒后又被拽走 */
  seenPairing: new Set(),
  /** @type {FileSession[]} */
  files: [],
  /** @type {{ key: string, count: number }[]} */
  clipboardPending: [],
  /** @type {PendingSend | null} 从系统菜单发起、等用户选目标的发送 */
  pick: null,
};

/** @param {string} id */
const $ = (id) => /** @type {HTMLElement} */ (document.getElementById(id));

/**
 * @param {string} tag
 * @param {Record<string, any>} [props]
 * @param {...(Node | string | null | undefined | false)} children
 */
function h(tag, props = {}, ...children) {
  const el = document.createElement(tag);
  for (const [key, value] of Object.entries(props)) {
    if (key === "class") el.className = value;
    else if (key.startsWith("on")) el.addEventListener(key.slice(2), value);
    else if (value === true) el.setAttribute(key, "");
    else if (value !== false && value != null) el.setAttribute(key, String(value));
  }
  for (const child of children) {
    if (child) el.append(child);
  }
  return el;
}

function selected() {
  return state.displays.find((d) => d.id === state.selectedId) ?? null;
}

/**
 * 本机读不到显示器时，心跳报告持棒的对端。
 * @param {DisplayView | null} d
 */
function remoteHolder(d) {
  if (!d || d.current_input != null) return null;
  return d.computers?.find((c) => c.active && !c.local) ?? null;
}

async function refresh() {
  state.loading = true;
  render();
  try {
    /** @type {Snapshot} */
    const snapshot = await invoke("list_displays");
    state.displays = snapshot.displays;
    state.configError = snapshot.config_error;
    if (!selected()) {
      const fallback = state.displays.find((d) => d.preferred) ?? state.displays[0];
      state.selectedId = fallback?.id ?? null;
    }
    for (const d of state.displays) {
      if (d.current_input != null) state.leftTo.delete(d.id);
    }
    if (state.notice && !state.notice.keep) state.notice = null;
  } catch (e) {
    state.notice = { tone: "err", text: String(e) };
  } finally {
    state.loading = false;
    render();
  }
}

/** @param {DisplayView} display @param {InputButton} input */
async function switchTo(display, input) {
  if (state.pending != null || input.active) return;
  state.pending = input.code;
  state.notice = { tone: "idle", text: `正在切到 ${input.label}…` };
  render();
  try {
    /** @type {SwitchOutcome} */
    const outcome = await invoke("switch_input", { id: display.id, code: input.code });
    state.pending = null;
    if (outcome.kind === "confirmed") {
      state.notice = { tone: "ok", text: `已切到 ${outcome.label}` };
      await refresh();
      state.notice = { tone: "ok", text: `已切到 ${outcome.label}` };
    } else {
      state.leftTo.set(display.id, input.code);
      display.current_input = null;
      display.current_label = null;
      for (const b of display.inputs) b.active = false;
      state.notice = { tone: "idle", text: `已切到 ${outcome.label}，画面已离开本机` };
    }
  } catch (e) {
    state.pending = null;
    state.notice = { tone: "err", text: String(e) };
  }
  render();
}

/** @param {ComputerView} c */
async function handOffTo(c) {
  if (c.local) {
    state.notice = { tone: "idle", text: c.active ? "画面已在本机" : "请在当前显示画面的电脑上操作" };
    render();
    return;
  }
  if (state.handingTo) return;
  state.handingTo = c.name;
    state.notice = { tone: "idle", text: `正在切换到 ${c.name}…` };
  render();
  try {
    await invoke("handoff_to", { peer: c.name });
  } catch (e) {
    state.notice = { tone: "err", text: String(e) };
  }
  setTimeout(() => {
    state.handingTo = null;
    refresh();
  }, 3000);
  render();
}

async function relearnInput() {
  closeMenu();
  try {
    await invoke("relearn_input");
    state.notice = { tone: "idle", text: "已清除本机接口，在本机操作一次键鼠即可重新识别" };
  } catch (e) {
    state.notice = { tone: "err", text: String(e) };
  }
  refresh();
}

function awaitingPair() {
  return state.devices?.sessions.some((s) => s.awaiting_me) ?? false;
}

/** @param {TabId} next */
function setTab(next) {
  if (state.tab === next) {
    applyTab();
    return;
  }
  if (state.tab === "local" && recordingHotkey) cancelHotkeyRecord();
  state.tab = next;
  applyTab();
}

function applyTab() {
  if (!TABS.includes(state.tab)) state.tab = "handoff";
  for (const id of TABS) {
    const selected = state.tab === id;
    const tab = $(`tab-${id}`);
    tab.setAttribute("aria-selected", String(selected));
    tab.tabIndex = selected ? 0 : -1;
    $(`pane-${id}`).hidden = !selected;
  }
  $("tab-devices").classList.toggle("mark", awaitingPair() || awaitingFile());
}

function render() {
  const d = selected();
  applyTab();
  renderHead(d);
  renderScreen(d);
  renderPorts(d);
  renderTransfers();
  renderDevices();
  renderFoot(d);
  renderMenu();
  renderUpdateRow();
  fit();
}

/**
 * @typedef {{ kind: "idle" | "available" | "installing", version: string | null }} UpdateStatus
 */

function renderUpdateRow() {
  const btn = /** @type {HTMLButtonElement} */ ($("pref-update"));
  const u = state.update;
  btn.disabled = u.kind === "installing" || u.checking;
  btn.classList.toggle("primary", u.kind === "available" && !u.checking);
  if (u.kind === "installing") btn.textContent = "正在升级…";
  else if (u.checking) btn.textContent = "正在检查更新…";
  else if (u.kind === "available" && u.version) btn.textContent = `升级到 v${u.version}`;
  else btn.textContent = "检查更新";
}

/** @param {UpdateStatus} status */
function applyUpdateStatus(status) {
  state.update = {
    kind: status.kind,
    version: status.version,
    checking: state.update.checking,
  };
}

async function refreshUpdate() {
  try {
    applyUpdateStatus(/** @type {UpdateStatus} */ (await invoke("update_status")));
  } catch (e) {
    logUpdateError(e);
  }
}

/** @param {unknown} e */
function invokeErrorText(e) {
  if (typeof e === "string") return e;
  if (e && typeof e === "object" && "message" in e && typeof e.message === "string" && e.message) {
    return e.message;
  }
  return String(e);
}

/** @param {unknown} e */
function logUpdateError(e) {
  const text = invokeErrorText(e);
  setNotice({ tone: "err", text, keep: true }, updateNoticeTtl(text));
}

let noticeTimer = 0;
let noticeGen = 0;

/**
 * @param {{ tone: "idle" | "ok" | "err", text: string, keep?: boolean }} notice
 * @param {number} [ttlMs]
 */
function setNotice(notice, ttlMs) {
  noticeGen += 1;
  const gen = noticeGen;
  clearTimeout(noticeTimer);
  state.notice = notice;
  if (ttlMs) {
    noticeTimer = setTimeout(() => {
      if (noticeGen !== gen) return;
      state.notice = null;
      render();
    }, ttlMs);
  }
}

/** @param {string} text */
function updateNoticeTtl(text) {
  if (
    text.includes("已是最新版本") ||
    text.includes("检查失败") ||
    text.includes("无法检查更新")
  ) {
    return NOTICE_TTL_MS;
  }
  return 0;
}

/** @param {string} text */
function isUpdateNoticeText(text) {
  return (
    text.includes("检查更新") ||
    text.includes("检查失败") ||
    text.includes("最新版本") ||
    text.includes("发现新版本") ||
    text.includes("无法安装更新") ||
    text.includes("更新包校验失败") ||
    text.includes("正在下载更新") ||
    text.includes("没有可用更新") ||
    text.includes("正在升级")
  );
}

async function onUpdateClick() {
  if (state.update.kind === "installing") return;
  if (state.update.checking) return;
  if (state.update.kind === "available") {
    state.update.kind = "installing";
    render();
    try {
      await invoke("install_update");
    } catch (e) {
      logUpdateError(e);
      await refreshUpdate();
      render();
    }
    return;
  }
  state.update.checking = true;
  setNotice({ tone: "idle", text: "正在检查更新…", keep: true });
  render();
  try {
    applyUpdateStatus(/** @type {UpdateStatus} */ (await invoke("check_update")));
    if (state.update.kind === "idle") {
      setNotice({ tone: "idle", text: "已是最新版本", keep: true }, NOTICE_TTL_MS);
    } else if (state.update.kind === "available" && state.update.version) {
      setNotice({
        tone: "idle",
        text: `发现新版本 v${state.update.version}`,
        keep: true,
      });
    }
  } catch (e) {
    logUpdateError(e);
  } finally {
    state.update.checking = false;
    render();
  }
}

/**
 * @typedef {{ name: string, hostname: string, hotkey: string, notify: boolean, clipboard_sync: boolean,
 *             autostart: boolean, files: { auto_accept: boolean, dir: string } }} PrefsView
 */

let recordingHotkey = false;
/** @type {string} */
let lastHotkey = "";

async function refreshPrefs() {
  try {
    /** @type {PrefsView} */
    const prefs = await invoke("prefs");
    const name = /** @type {HTMLInputElement} */ ($("pref-name"));
    const hotkey = $("pref-hotkey");
    const autostart = /** @type {HTMLInputElement} */ ($("pref-autostart"));
    const notify = /** @type {HTMLInputElement} */ ($("pref-notify"));
    const clipboard = /** @type {HTMLInputElement} */ ($("pref-clipboard"));
    name.placeholder = prefs.hostname;
    if (document.activeElement !== name) name.value = prefs.name;
    lastHotkey = prefs.hotkey;
    if (!recordingHotkey) {
      hotkey.textContent = prefs.hotkey;
      hotkey.classList.remove("recording");
    }
    autostart.checked = prefs.autostart;
    notify.checked = prefs.notify;
    clipboard.checked = prefs.clipboard_sync;
    /** @type {HTMLInputElement} */ ($("pref-auto-accept")).checked = prefs.files.auto_accept;
    showReceiveDir(prefs.files.dir);
  } catch (e) {
    state.notice = { tone: "err", text: String(e) };
    render();
  }
}

async function saveName() {
  const name = /** @type {HTMLInputElement} */ ($("pref-name"));
  try {
    await invoke("set_name", { name: name.value });
    state.agent = await invoke("agent_status");
    await Promise.all([refresh(), refreshDevices(), refreshPrefs()]);
  } catch (e) {
    state.notice = { tone: "err", text: String(e) };
    render();
  }
}

async function saveNotify() {
  const on = /** @type {HTMLInputElement} */ ($("pref-notify")).checked;
  try {
    await invoke("set_notify", { notify: on });
  } catch (e) {
    state.notice = { tone: "err", text: String(e) };
    render();
    refreshPrefs();
  }
}

async function saveClipboard() {
  const on = /** @type {HTMLInputElement} */ ($("pref-clipboard")).checked;
  try {
    await invoke("set_clipboard_sync", { clipboardSync: on });
  } catch (e) {
    state.notice = { tone: "err", text: String(e) };
    render();
    refreshPrefs();
  }
}

/** @param {string} dir */
function showReceiveDir(dir) {
  const btn = $("pref-receive-dir");
  btn.textContent = dir ? dir.split(/[\\/]/).filter(Boolean).pop() ?? dir : "下载";
  btn.title = dir;
}

async function saveAutoAccept() {
  const on = /** @type {HTMLInputElement} */ ($("pref-auto-accept")).checked;
  try {
    await invoke("set_file_auto_accept", { on });
  } catch (e) {
    state.notice = { tone: "err", text: String(e) };
    render();
    refreshPrefs();
  }
}

async function changeReceiveDir() {
  try {
    /** @type {string | null} */
    const dir = await invoke("set_receive_dir");
    if (dir != null) showReceiveDir(dir);
  } catch (e) {
    state.notice = { tone: "err", text: String(e) };
    render();
  }
}

async function saveAutostart() {
  const on = /** @type {HTMLInputElement} */ ($("pref-autostart")).checked;
  try {
    const enabled = await invoke("set_autostart", { on });
    /** @type {HTMLInputElement} */ ($("pref-autostart")).checked = enabled;
  } catch (e) {
    state.notice = { tone: "err", text: String(e) };
    render();
    refreshPrefs();
  }
}

/** @param {KeyboardEvent} e */
function comboFromEvent(e) {
  if (["Shift", "Control", "Alt", "Meta"].includes(e.key)) return null;
  const mods = [];
  if (e.ctrlKey) mods.push("Ctrl");
  if (e.altKey) mods.push("Alt");
  if (e.metaKey) mods.push("Super");
  if (e.shiftKey) mods.push("Shift");
  if (!e.ctrlKey && !e.altKey && !e.metaKey) return null;
  const key = keyName(e);
  if (!key) return null;
  mods.push(key);
  return mods.join("+");
}

/** @param {KeyboardEvent} e */
function keyName(e) {
  const code = e.code;
  if (code === "Space") return "Space";
  if (code.startsWith("Key") && code.length === 4) return code.slice(3);
  if (code.startsWith("Digit") && code.length === 6) return code.slice(5);
  if (/^F\d{1,2}$/.test(code)) return code;
  /** @type {Record<string, string>} */
  const named = {
    ArrowUp: "Up",
    ArrowDown: "Down",
    ArrowLeft: "Left",
    ArrowRight: "Right",
    Enter: "Return",
    Tab: "Tab",
    Backspace: "Backspace",
    Minus: "Minus",
    Equal: "Equal",
  };
  return named[code] ?? null;
}

async function startHotkeyRecord() {
  if (recordingHotkey) return;
  recordingHotkey = true;
  const btn = $("pref-hotkey");
  btn.textContent = "按下新热键…";
  btn.classList.add("recording");
  try {
    await invoke("suspend_hotkey");
  } catch (e) {
    recordingHotkey = false;
    state.notice = { tone: "err", text: String(e) };
    render();
  }
}

async function cancelHotkeyRecord() {
  if (!recordingHotkey) return;
  recordingHotkey = false;
  $("pref-hotkey").classList.remove("recording");
  try {
    await invoke("resume_hotkey");
  } catch (e) {
    state.notice = { tone: "err", text: String(e) };
  }
  await refreshPrefs();
}

/** @param {string} combo */
async function commitHotkey(combo) {
  recordingHotkey = false;
  try {
    const hotkey = await invoke("set_hotkey", { hotkey: combo });
    lastHotkey = hotkey;
    $("pref-hotkey").textContent = hotkey;
    $("pref-hotkey").classList.remove("recording");
    state.agent = await invoke("agent_status");
    render();
  } catch (e) {
    state.notice = { tone: "err", text: String(e) };
    render();
    await cancelHotkeyRecord();
  }
}

async function refreshDevices() {
  try {
    state.devices = await invoke("list_devices");
    state.clipboardPending = await invoke("clipboard_pending").catch(() => []);
    state.devicesError = null;
    for (const c of state.devices.candidates) {
      if (state.devices.sessions.some((s) => s.peer === c.name)) state.pairing.delete(c.address);
    }
    const waiting = state.devices.sessions.filter((s) => s.awaiting_me);
    const fresh = waiting.some((s) => !state.seenPairing.has(s.id));
    state.seenPairing = new Set(waiting.map((s) => s.id));
    if (fresh && state.tab === "handoff") state.tab = "devices";
  } catch (e) {
    state.devices = null;
    state.devicesError = String(e);
  }
  render();
}

let devicesTimer = 0;
function scheduleDevices() {
  clearTimeout(devicesTimer);
  devicesTimer = setTimeout(refreshDevices, 80);
}

/** @param {PairingSession} session @param {boolean} accept */
async function decide(session, accept) {
  try {
    await invoke("confirm_pairing", { session: session.id, accept });
  } catch (e) {
    state.notice = { tone: "err", text: String(e) };
  }
  scheduleDevices();
}

/** @param {Candidate} c */
async function pair(c) {
  state.pairing.add(c.address);
  state.notice = { tone: "idle", text: `正在连接 ${c.name}…` };
  render();
  try {
    await invoke("pair_device", { address: c.address, fingerprint: c.fingerprint });
  } catch (e) {
    state.pairing.delete(c.address);
    state.notice = { tone: "err", text: String(e) };
  }
  setTimeout(() => {
    state.pairing.delete(c.address);
    render();
  }, 8000);
  render();
}

/** @param {PairedPeer} peer */
async function unpair(peer) {
  if (state.removing !== peer.fingerprint) {
    state.removing = peer.fingerprint;
    render();
    return;
  }
  state.removing = null;
  try {
    await invoke("unpair_device", { fingerprint: peer.fingerprint });
    state.notice = { tone: "idle", text: `已取消与 ${peer.name} 的配对` };
  } catch (e) {
    state.notice = { tone: "err", text: String(e) };
  }
  scheduleDevices();
}

/* ---------- 文件快传 ---------- */

/** 发送方的结果卡片停留多久后自动收起。 */
const SEND_RESULT_TTL_MS = 5000;
const REVEAL_LABEL = navigator.userAgent.includes("Windows") ? "在资源管理器中显示" : "在访达中显示";

function awaitingFile() {
  return state.files.some((s) => s.direction === "receive" && s.phase === "pending");
}

/** @param {number} n */
function fmtBytes(n) {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  let v = n;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i += 1;
  }
  return `${i === 0 ? v : v.toFixed(v >= 100 ? 0 : 1)} ${units[i]}`;
}

/** @param {OfferSummary} s */
function summaryText(s) {
  const parts = [`${s.files} 个文件`];
  if (s.dirs > 0) parts.push(`${s.dirs} 个文件夹`);
  parts.push(fmtBytes(s.total_bytes));
  return parts.join(" · ");
}

/** @type {Record<string, string>} */
const REJECT_TEXT = {
  declined: "对方拒绝了接收",
  no_space: "对方磁盘空间不足",
  unavailable: "对方没有打开面板，无法确认接收",
  error: "对方无法接收",
};

/** @type {Map<string, number>} */
const dismissTimers = new Map();

/** @param {FileSession} s */
const sessionKey = (s) => `${s.peer}/${s.id}`;

async function refreshFiles() {
  try {
    state.files = await invoke("file_sessions");
  } catch {
    state.files = [];
  }
  for (const s of state.files) {
    const key = sessionKey(s);
    const fullySent = s.phase === "done" && (s.failed ?? 0) === 0;
    const transient = s.direction === "send" && (fullySent || s.phase === "canceled");
    if (transient && !dismissTimers.has(key)) {
      dismissTimers.set(
        key,
        setTimeout(() => {
          dismissTimers.delete(key);
          dismissFile(s);
        }, SEND_RESULT_TTL_MS),
      );
    }
  }
  render();
  if (document.hasFocus()) markFilesSeen();
}

let filesTimer = 0;
function scheduleFiles() {
  clearTimeout(filesTimer);
  filesTimer = setTimeout(refreshFiles, 60);
}

function markFilesSeen() {
  const unseen = state.files.some((s) => s.direction === "receive" && s.phase === "done" && !s.seen);
  if (unseen) invoke("mark_files_seen").catch(() => {});
}

async function refreshPick() {
  try {
    state.pick = await invoke("pending_send");
  } catch {
    state.pick = null;
  }
  render();
}

/** @param {string} cmd @param {FileSession} s */
async function fileAction(cmd, s) {
  try {
    await invoke(cmd, { peer: s.peer, id: s.id });
  } catch (e) {
    state.notice = { tone: "err", text: String(e) };
  }
  scheduleFiles();
}

/** @param {FileSession} s */
const dismissFile = (s) => fileAction("dismiss_file", s);

/** @param {string} peer */
async function switchToPeer(peer) {
  try {
    await invoke("handoff_to", { peer });
  } catch (e) {
    state.notice = { tone: "err", text: String(e) };
    render();
  }
}

/** @param {number} done @param {number} total */
function progressBar(done, total) {
  const pct = total > 0 ? Math.min(100, Math.round((done / total) * 100)) : 100;
  const bar = h("div", { class: "bar" }, h("i", {}));
  /** @type {HTMLElement} */ (bar.firstChild).style.width = `${pct}%`;
  return { bar, pct };
}

/** @param {FileSession} s */
function transferCard(s) {
  const send = s.direction === "send";
  const names = s.summary.names.join("、");
  const summary = summaryText(s.summary);
  const button = (/** @type {string} */ label, /** @type {() => void} */ onclick, primary = false) =>
    h("button", { class: primary ? "btn primary" : "btn", onclick }, label);
  const cancel = button("取消", () => fileAction("cancel_file", s));
  const reveal = (/** @type {string | null | undefined} */ path) =>
    path ? button(REVEAL_LABEL, () => invoke("reveal_path", { path })) : null;
  /** @type {string} */
  let tone = "";
  /** @type {string} */
  let kicker = "";
  /** @type {string} */
  let title = "";
  /** @type {string} */
  let meta = summary;
  /** @type {number | null} */
  let pct = null;
  /** @type {HTMLElement | null} */
  let bar = null;
  /** @type {(Node | null)[]} */
  let actions = [];
  let terminal = false;
  switch (s.phase) {
    case "waking":
      tone = "wait";
      kicker = "唤醒";
      title = `正在唤醒 ${s.peer}`;
      bar = h("div", { class: "bar wait" }, h("i", { "aria-hidden": "true" }));
      actions = [cancel];
      break;
    case "offered":
      tone = "wait";
      kicker = "等待";
      title = `等 ${s.peer} 接收`;
      bar = h("div", { class: "bar wait" }, h("i", { "aria-hidden": "true" }));
      actions = [button("切过去", () => switchToPeer(s.peer), true), cancel];
      break;
    case "pending":
      tone = "live";
      kicker = "待确认";
      title = `${s.peer} 发来文件`;
      actions = [
        button("拒绝", () => fileAction("reject_file", s)),
        button("接收", () => fileAction("accept_file", s), true),
      ];
      break;
    case "transferring": {
      const p = progressBar(s.done_bytes, s.summary.total_bytes);
      tone = "live";
      kicker = send ? "发送中" : "接收中";
      title = send ? `正在发给 ${s.peer}` : `正在接收 ${s.peer} 的文件`;
      meta = `${fmtBytes(s.done_bytes)} / ${fmtBytes(s.summary.total_bytes)}`;
      pct = p.pct;
      bar = p.bar;
      actions = [cancel];
      break;
    }
    case "done": {
      const received = s.received ?? 0;
      const failed = s.failed ?? 0;
      const partial = failed > 0;
      tone = partial ? "err" : "ok";
      terminal = true;
      if (send) {
        kicker = partial ? "部分送达" : "已送达";
        title = `已发给 ${s.peer}`;
        meta = partial ? `${received} / ${received + failed} 个文件，其余失败` : `${received} 个文件`;
      } else {
        kicker = partial ? "部分收到" : "已收到";
        title = `来自 ${s.peer}`;
        meta = partial ? `${failed} 个未能收下` : `${received} 个文件`;
        actions = [reveal(s.location)];
      }
      break;
    }
    case "failed":
      tone = "err";
      terminal = true;
      kicker = send ? "未送达" : "未收下";
      title = send ? `没能发给 ${s.peer}` : `没能收下 ${s.peer} 的文件`;
      meta = s.reason ?? "";
      actions = [reveal(s.reveal), s.can_retry ? button("重试", () => retryFile(s), true) : null];
      break;
    case "rejected":
      tone = "err";
      terminal = true;
      kicker = "已拒绝";
      title = send ? `${s.peer} 未接收` : "未接收";
      meta = send ? (REJECT_TEXT[s.reason ?? "error"] ?? REJECT_TEXT.error) : `来自 ${s.peer}`;
      if (!send && s.reason === "no_space") meta = "磁盘空间不足，未接收";
      break;
    case "canceled":
      tone = "wait";
      terminal = true;
      kicker = "已取消";
      title = s.by_peer ? (send ? `${s.peer} 取消了接收` : `${s.peer} 取消了发送`) : "已取消";
      meta = summary;
      break;
  }
  const close = terminal
    ? h("button", { class: "btn ghost xfer-close", title: "收起", onclick: () => dismissFile(s) }, "×")
    : null;
  return h(
    "div",
    { class: ["transfer", tone].filter(Boolean).join(" ") },
    h(
      "div",
      { class: "xfer-top" },
      h("span", { class: "kicker" }, kicker),
      h("b", { title }, title),
      pct != null ? h("span", { class: "pct" }, `${pct}%`) : null,
      close,
    ),
    names ? h("div", { class: "xfer-names", title: names }, names) : null,
    bar,
    h("div", { class: "xfer-meta", title: meta }, meta),
    actions.some(Boolean) ? h("div", { class: "xfer-actions" }, ...actions) : null,
  );
}

/** @param {FileSession} s */
function retryFile(s) {
  fileAction("retry_file", s);
}

function pickCard() {
  const pick = state.pick;
  if (!pick) return null;
  const names = pick.names.join("、") + (pick.count > pick.names.length ? ` 等 ${pick.count} 项` : "");
  const targets = pick.targets.map((t, i) =>
    h(
      "button",
      {
        class: i === 0 ? "btn primary" : "btn",
        title: t.online ? "在线" : "离线，将先唤醒",
        onclick: async () => {
          state.pick = null;
          render();
          await invoke("send_pending", { peer: t.name }).catch((e) => {
            state.notice = { tone: "err", text: String(e) };
          });
          scheduleFiles();
        },
      },
      t.name,
    ),
  );
  return h(
    "div",
    { class: "pair-request transfer" },
    h("div", { class: "pair-head" }, h("span", {}, "发送到"), h("b", {}, names)),
    h(
      "div",
      { class: "pair-foot targets" },
      ...targets,
      h(
        "button",
        {
          class: "btn ghost",
          onclick: () => {
            state.pick = null;
            invoke("discard_pending").catch(() => {});
            render();
          },
        },
        "取消",
      ),
    ),
  );
}

function renderTransfers() {
  const box = $("transfers");
  const cards = [pickCard(), ...state.files.map(transferCard)].filter(Boolean);
  box.replaceChildren(.../** @type {Node[]} */ (cards));
  box.hidden = cards.length === 0;
}

/** @type {string | null} 右键菜单对应的设备 */
let ctxPeer = null;

/** @param {MouseEvent} e @param {string} peer */
function openSendMenu(e, peer) {
  e.preventDefault();
  e.stopPropagation();
  closeMenu();
  ctxPeer = peer;
  const menu = $("ctx-menu");
  menu.replaceChildren(
    h("div", { class: "menu-label" }, `发送到 ${peer}`),
    h("button", { role: "menuitem", onclick: () => pickAndSend(false) }, h("span", { class: "check" }), "发送文件…"),
    h("button", { role: "menuitem", onclick: () => pickAndSend(true) }, h("span", { class: "check" }), "发送文件夹…"),
  );
  menu.hidden = false;
  const panel = $("panel").getBoundingClientRect();
  const rect = menu.getBoundingClientRect();
  const x = Math.min(e.clientX - panel.left, panel.width - rect.width - 8);
  const y = Math.min(e.clientY - panel.top, panel.height - rect.height - 8);
  menu.style.left = `${Math.max(8, x)}px`;
  menu.style.top = `${Math.max(8, y)}px`;
}

function closeSendMenu() {
  $("ctx-menu").hidden = true;
}

/** @param {boolean} folder */
async function pickAndSend(folder) {
  closeSendMenu();
  const peer = ctxPeer;
  if (!peer) return;
  try {
    await invoke("pick_and_send", { peer, folder });
  } catch (e) {
    state.notice = { tone: "err", text: String(e) };
    render();
  }
  scheduleFiles();
}

/** @param {{ fingerprint: string, name: string }} peer */
function clipCount(peer) {
  const key = peer.fingerprint || peer.name;
  return state.clipboardPending.find((row) => row.key === key)?.count ?? 0;
}

/** @param {boolean} online @param {number} count */
function presence(online, count) {
  const base = online ? "在线" : "离线";
  return count > 0 ? `${base} · ${count} 条待发出` : base;
}

/**
 * @param {{ key: string, count: number }[]} a
 * @param {{ key: string, count: number }[]} b
 */
function samePending(a, b) {
  return a.length === b.length && a.every((row, i) => row.key === b[i].key && row.count === b[i].count);
}

function renderDevices() {
  const box = $("pairing");
  const list = $("device-list");
  box.replaceChildren();
  list.replaceChildren();
  const devices = state.devices;
  $("devices-sub").textContent = devices ? `本机 ${devices.me}` : "局域网";
  if (!devices) {
    const text = state.devicesError ?? state.agent?.error ?? "正在启动…";
    list.append(h("div", { class: "empty" }, text));
    return;
  }
  for (const s of devices.sessions) {
    const about = [s.platform ? PLATFORM_NAMES[s.platform] : null, s.ip].filter(Boolean).join(" · ");
    box.append(
      h(
        "div",
        { class: "pair-request" },
        h("div", { class: "pair-head" }, h("span", {}, "请求连接"), h("b", {}, s.peer)),
        h(
          "div",
          { class: "pair-foot" },
          h("small", {}, s.awaiting_me ? about || "同意后即可在两台电脑间切换" : "等待对方同意…"),
          s.awaiting_me ? h("button", { class: "btn", onclick: () => decide(s, false) }, "拒绝") : null,
          s.awaiting_me ? h("button", { class: "btn primary", onclick: () => decide(s, true) }, "同意") : null,
        ),
      ),
    );
  }
  for (const { peer, online } of devices.paired) {
    const sub = [peer.last_ip, peer.wol_mac].filter(Boolean).join(" · ") || "尚未记下地址";
    const pending = clipCount(peer);
    const removing = state.removing === peer.fingerprint;
    list.append(
      h(
        "div",
        {
          class: "device" + (online ? " online" : ""),
          title: "右键发送文件",
          oncontextmenu: (/** @type {MouseEvent} */ e) => openSendMenu(e, peer.name),
        },
        h("i", { class: "dot" }),
        h("div", { class: "who" }, h("b", {}, peer.name), h("small", {}, sub)),
        h("span", { class: "state" + (pending ? " pending" : "") }, presence(online, pending)),
        h(
          "button",
          { class: "btn ghost" + (removing ? " danger" : ""), title: "取消配对", onclick: () => unpair(peer) },
          removing ? "确认移除" : "移除",
        ),
      ),
    );
  }
  for (const c of devices.candidates) {
    if (devices.sessions.some((s) => s.peer === c.name)) continue;
    const busy = state.pairing.has(c.address);
    list.append(
      h(
        "div",
        { class: "device candidate" },
        h("i", { class: "dot" }),
        h("div", { class: "who" }, h("b", {}, c.name), h("small", {}, c.address)),
        h("span", { class: "state" }, c.fingerprint ? "可配对" : "配置中的对端"),
        h("button", { class: "btn", disabled: busy, onclick: () => pair(c) }, busy ? "连接中" : "连接"),
      ),
    );
  }
  if (devices.paired.length === 0 && devices.candidates.length === 0 && devices.sessions.length === 0) {
    list.append(h("div", { class: "empty" }, "局域网里还没有发现其他开着 DeskBaton 的设备"));
  }
}

/** @param {DisplayView | null} d */
function renderHead(d) {
  $("name").textContent = d?.name ?? "DeskBaton";
  $("subtitle").textContent = d
    ? d.subtitle
    : state.loading
      ? "正在读取显示器…"
      : "未发现支持 DDC/CI 的外接显示器";
}

/** @param {DisplayView | null} d */
function renderScreen(d) {
  const bezel = $("bezel");
  const res = $("res");
  const sub = $("screen-sub");
  const screen = d?.screen;
  res.replaceChildren();
  res.classList.toggle("unknown", !screen);
  bezel.classList.toggle("portrait", !!screen && screen.height > screen.width);
  bezel.style.aspectRatio = screen ? `${screen.width} / ${screen.height}` : "16 / 9";
  if (screen) {
    res.append(String(screen.width), h("i", {}, "×"), String(screen.height));
    const parts = [];
    if (screen.hidpi) parts.push("HiDPI", `渲染 ${screen.pixel_width}×${screen.pixel_height}`);
    if (screen.scale_percent != null && screen.scale_percent !== 100) {
      parts.push(`缩放 ${screen.scale_percent}%`);
    }
    if (screen.refresh_hz != null) parts.push(`${screen.refresh_hz} Hz`);
    sub.replaceChildren(...parts.map((p) => h("span", {}, p)));
  } else {
    res.textContent = d ? "未知分辨率" : state.loading ? "读取中" : "无显示器";
    sub.textContent = "";
  }

  const away = !!d && d.current_input == null;
  bezel.classList.toggle("dark", !d || away);
  $("now").classList.toggle("away", !d || away);
  const owners = d?.inputs.find((i) => i.active)?.owners ?? [];
  const current = [d?.current_label ?? "—", ownerNames(owners)].filter(Boolean).join(" · ");
  const holder = remoteHolder(d);
  $("now-label").textContent =
    d && !away
      ? current
      : holder
        ? [holder.port, holder.name].filter(Boolean).join(" · ")
        : d
          ? "其它输入"
          : "—";
}

/** @param {DisplayView | null} d */
function renderPorts(d) {
  const ports = $("ports");
  ports.replaceChildren();
  const computers = d?.computers;
  ports.classList.toggle("computers", !!computers);
  $("ports-title").textContent = computers ? "电脑" : "输入源";
  $("ports-sub").textContent = computers ? "点击即可切换" : "DDC/CI · VCP 0x60";
  if (!d) {
    ports.append(h("div", { class: "empty" }, state.loading ? "正在读取…" : "没有可切换的显示器"));
    return;
  }
  if (computers) {
    renderComputers(ports, computers);
    return;
  }
  const leftTo = state.leftTo.get(d.id);
  for (const input of d.inputs) {
    const pending = state.pending === input.code;
    const switched = !input.active && leftTo === input.code;
    const code = `0x${input.code.toString(16).toUpperCase().padStart(2, "0")}`;
    const platforms = ownerPlatforms(input.owners);
    const who = input.owners.map((o) => (o.platform ? `${o.name}（${PLATFORM_NAMES[o.platform] ?? "其它系统"}）` : o.name));
    const button = h(
      "button",
      {
        class: ["port", input.active || switched ? "on" : "", pending ? "pending" : ""].join(" "),
        disabled: state.pending != null,
        title: [code, ...who].join(" · "),
        onclick: () => switchTo(d, input),
      },
      h("i", { class: "led" }),
      switched ? h("span", { class: "tag" }, "已切到") : null,
    );
    button.insertAdjacentHTML("beforeend", `<svg viewBox="0 0 30 14">${PORT_PATHS[input.kind]}</svg>`);
    button.append(input.label);
    if (input.owners.length > 0) {
      button.append(
        h("span", { class: "owner" }, h("b", {}, ownerNames(input.owners)), platforms ? h("small", {}, platforms) : null),
      );
    }
    ports.append(button);
  }
}

/** @param {HTMLElement} box @param {ComputerView[]} computers */
function renderComputers(box, computers) {
  for (const c of computers) {
    const status = c.local ? "本机" : c.online ? "在线" : "离线";
    const platform = c.platform ? PLATFORM_NAMES[c.platform] ?? "其它系统" : null;
    const pending = state.handingTo === c.name;
    const button = h(
      "button",
      {
        class: ["port", "computer", c.active ? "on" : "", pending ? "pending" : ""].join(" "),
        disabled: state.handingTo != null,
        title: [c.name, platform, c.port ?? "接口未识别", c.local ? null : "右键发送文件"].filter(Boolean).join(" · "),
        onclick: () => handOffTo(c),
        oncontextmenu: (/** @type {MouseEvent} */ e) => (c.local ? e.preventDefault() : openSendMenu(e, c.name)),
      },
      h("i", { class: "led" }),
    );
    button.insertAdjacentHTML("beforeend", `<svg viewBox="0 0 30 14">${COMPUTER_PATH}</svg>`);
    button.append(
      h("span", { class: "name" }, c.name),
      h("span", { class: "owner" }, h("b", {}, status), h("small", {}, c.port ?? "未识别")),
    );
    box.append(button);
  }
}

/** @param {DisplayView | null} d */
function renderFoot(d) {
  const status = $("status");
  const holder = remoteHolder(d);
  let tone = "idle";
  let text;
  if (state.notice) {
    ({ tone, text } = state.notice);
  } else if (state.loading) {
    text = "正在读取…";
  } else if (state.configError) {
    tone = "err";
    text = state.configError;
  } else if (state.agent?.error) {
    tone = "err";
    text = state.agent.error;
  } else if (!d) {
    text = "未发现显示器";
  } else if (d.current_input != null) {
    tone = "ok";
    text = "DDC 正常";
  } else if (holder) {
    text = `画面在 ${holder.name}，本机读不到输入源`;
  } else {
    text = "本机不是当前输入，读不到输入源";
  }
  status.dataset.tone = tone;
  status.textContent = text;
  status.title = text;
  const ver = $("version");
  ver.textContent = state.version ? `v${state.version}` : "";
  ver.title = state.version ? `DeskBaton ${state.version}` : "";
}

function renderMenu() {
  const menu = $("menu");
  const items = [];
  if (state.displays.length > 0) {
    items.push(h("div", { class: "menu-label" }, "显示器"));
    for (const d of state.displays) {
      items.push(
        h(
          "button",
          {
            role: "menuitem",
            onclick: () => {
              state.selectedId = d.id;
              localStorage.setItem(SELECTED_KEY, d.id);
              state.notice = null;
              closeMenu();
              render();
            },
          },
          h("span", { class: "check" }, d.id === state.selectedId ? "●" : ""),
          d.name,
          d.serial ? h("small", {}, d.serial.slice(-6)) : null,
        ),
      );
    }
    items.push(h("hr"));
  }
  if (state.agent?.name && !state.agent.input_code_configured) {
    items.push(
      h("button", { role: "menuitem", onclick: relearnInput }, h("span", { class: "check" }), "重新识别本机接口"),
    );
  }
  items.push(
    h("button", { role: "menuitem", onclick: () => (closeMenu(), refresh()) }, h("span", { class: "check" }), "刷新"),
    h("button", { role: "menuitem", onclick: () => invoke("quit") }, h("span", { class: "check" }), "退出"),
  );
  menu.replaceChildren(...items);
}

function closeMenu() {
  $("menu").hidden = true;
  $("more").setAttribute("aria-expanded", "false");
  fit();
}

let fitFrame = 0;

function applyPlatform(p) {
  document.documentElement.classList.add(`platform-${p}`);
}

invoke("host_platform").then(applyPlatform).catch(() => {});

function requestFit() {
  cancelAnimationFrame(fitFrame);
  fitFrame = requestAnimationFrame(() => {
    const panelRect = $("panel").getBoundingClientRect();
    const menu = $("menu");
    const menuRect = menu.getBoundingClientRect();
    const bottom = menu.hidden
      ? panelRect.height
      : Math.max(panelRect.height, menuRect.bottom - panelRect.top);
    const height = Math.ceil(bottom) || 480;
    invoke("fit_panel", { height }).catch(() => {
      invoke("fit_panel", { height }).catch(() => {});
    });
  });
}

function fit() {
  requestFit();
}

function reveal() {
  const panel = $("panel");
  panel.classList.remove("reveal");
  void panel.offsetWidth;
  panel.classList.add("reveal");
}

$("tabs").addEventListener("click", (e) => {
  const btn = /** @type {HTMLElement | null} */ (/** @type {HTMLElement} */ (e.target).closest("[data-tab]"));
  if (btn?.dataset.tab) setTab(/** @type {TabId} */ (btn.dataset.tab));
});

$("tabs").addEventListener("keydown", (e) => {
  if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
  const tabs = TABS;
  const i = tabs.indexOf(state.tab);
  const next = tabs[(i + (e.key === "ArrowRight" ? 1 : tabs.length - 1)) % tabs.length];
  setTab(next);
  $(`tab-${next}`).focus();
  e.preventDefault();
});

$("more").addEventListener("click", (e) => {
  e.stopPropagation();
  const menu = $("menu");
  menu.hidden = !menu.hidden;
  $("more").setAttribute("aria-expanded", String(!menu.hidden));
  fit();
});

document.addEventListener("click", (e) => {
  if (!$("menu").hidden && !$("menu").contains(/** @type {Node} */ (e.target))) closeMenu();
  if (!$("ctx-menu").hidden && !$("ctx-menu").contains(/** @type {Node} */ (e.target))) closeSendMenu();
});

document.addEventListener("contextmenu", closeSendMenu);

document.addEventListener("keydown", (e) => {
  if (recordingHotkey) {
    e.preventDefault();
    e.stopPropagation();
    if (e.key === "Escape") {
      cancelHotkeyRecord();
      return;
    }
    const combo = comboFromEvent(e);
    if (combo) commitHotkey(combo);
    return;
  }
  if (e.key === "Escape") {
    closeMenu();
    closeSendMenu();
  }
});

$("pref-name").addEventListener("change", saveName);
$("pref-name").addEventListener("keydown", (e) => {
  if (e.key === "Enter") {
    e.preventDefault();
    /** @type {HTMLInputElement} */ ($("pref-name")).blur();
  }
});
$("pref-hotkey").addEventListener("click", startHotkeyRecord);
$("pref-notify").addEventListener("change", saveNotify);
$("pref-clipboard").addEventListener("change", saveClipboard);
$("pref-autostart").addEventListener("change", saveAutostart);
$("pref-auto-accept").addEventListener("change", saveAutoAccept);
$("pref-receive-dir").addEventListener("click", changeReceiveDir);
$("pref-update").addEventListener("click", onUpdateClick);
window.addEventListener("blur", () => {
  if (recordingHotkey) cancelHotkeyRecord();
  closeSendMenu();
});
window.addEventListener("focus", markFilesSeen);

tauri.event.listen("panel-will-show", () => {
  closeMenu();
  closeSendMenu();
  requestFit();
  if (state.pending == null) refresh();
  refreshDevices();
  refreshPrefs();
  refreshFiles();
  refreshPick();
});

tauri.event.listen("panel-shown", () => {
  closeMenu();
  closeSendMenu();
  reveal();
  if (!document.documentElement.classList.contains("platform-windows")) {
    if (state.pending == null) refresh();
    refreshDevices();
    refreshPrefs();
    refreshFiles();
    refreshPick();
  }
});

tauri.event.listen("files-changed", scheduleFiles);

tauri.event.listen("files-focus", () => {
  refreshFiles();
  refreshPick();
});

tauri.event.listen("send-pick", refreshPick);

let displaysTimer = 0;
// 合并短时间内的多次变化，少读 DDC；正在读取或切换时顺延，不丢掉这次变化。
function scheduleDisplays() {
  clearTimeout(displaysTimer);
  displaysTimer = setTimeout(() => {
    if (state.pending != null || state.loading) scheduleDisplays();
    else refresh();
  }, 600);
}

tauri.event.listen("devices-changed", () => {
  scheduleDevices();
  // 设备上下线、学到接口、本机或对端持棒状态变化时，显示器和「电脑」卡片都要更新。
  scheduleDisplays();
});

tauri.event.listen("notice", (/** @type {{ payload: string }} */ e) => {
  const text = e.payload;
  setNotice({ tone: "idle", text, keep: isUpdateNoticeText(text) }, updateNoticeTtl(text));
  render();
});

tauri.event.listen("update-available", (/** @type {{ payload: string }} */ e) => {
  state.update.kind = "available";
  state.update.version = e.payload;
  render();
});

tauri.event.listen("update-progress", () => {
  state.update.kind = "installing";
  render();
});

invoke("agent_status")
  .then((/** @type {AgentStatus} */ status) => {
    state.agent = status;
    render();
  })
  .catch(() => {});

invoke("app_version")
  .then((/** @type {string} */ v) => {
    state.version = v;
    render();
  })
  .catch(() => {});

refresh();
refreshDevices();
setInterval(async () => {
  try {
    const rows = await invoke("clipboard_pending");
    if (samePending(state.clipboardPending, rows)) return;
    state.clipboardPending = rows;
    if (state.devices) renderDevices();
  } catch {
    /* 服务还没起来时没有发件箱 */
  }
}, 2000);
refreshPrefs();
refreshFiles();
refreshPick();
refreshUpdate().then(() => render());
