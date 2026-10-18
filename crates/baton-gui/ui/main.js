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
 */

const tauri = /** @type {any} */ (window).__TAURI__;
/** @type {<T>(cmd: string, args?: object) => Promise<T>} */
const invoke = tauri.core.invoke;

const SELECTED_KEY = "deskbaton.display";

/** @typedef {"handoff" | "devices" | "local" | "clipboard"} TabId */

/** @type {TabId[]} */
const TABS = ["handoff", "devices", "clipboard", "local"];

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
  /** @type {{ tone: "idle" | "ok" | "err", text: string } | null} 覆盖默认状态行的一次性提示 */
  notice: null,
  /** @type {AgentStatus | null} */
  agent: null,
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
  clipboardTab: true,
  /** @type {OutboxItem[]} */
  outbox: [],
  /** @type {number | null} */
  expandedSeq: null,
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
    state.notice = null;
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
  fit();
}

function visibleTabs() {
  return state.clipboardTab ? TABS : TABS.filter((id) => id !== "clipboard");
}

function applyTab() {
  const tabs = visibleTabs();
  if (!tabs.includes(state.tab)) state.tab = "handoff";
  $("tab-clipboard").hidden = !state.clipboardTab;
  for (const id of TABS) {
    const selected = state.tab === id;
    const tab = $(`tab-${id}`);
    tab.setAttribute("aria-selected", String(selected));
    tab.tabIndex = selected ? 0 : -1;
    $(`pane-${id}`).hidden = !selected;
  }
  $("tab-devices").classList.toggle("mark", awaitingPair());
}

function render() {
  const d = selected();
  applyTab();
  renderHead(d);
  renderScreen(d);
  renderPorts(d);
  renderDevices();
  renderClipboard();
  renderFoot(d);
  renderMenu();
  fit();
}

/**
 * @typedef {{ name: string, hostname: string, hotkey: string, notify: boolean, clipboard_sync: boolean,
 *             clipboard_tab: boolean, clipboard_outbox_limit: number, autostart: boolean }} PrefsView
 * @typedef {{ seq: number, peer: string, format: string, text: string | null, png_base64: string | null }} OutboxItem
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
    /** @type {HTMLInputElement} */ ($("pref-clipboard-tab")).checked = prefs.clipboard_tab;
    /** @type {HTMLInputElement} */ ($("pref-outbox-limit")).value = String(prefs.clipboard_outbox_limit);
    const tabWas = state.clipboardTab;
    state.clipboardTab = prefs.clipboard_tab;
    if (tabWas !== state.clipboardTab) applyTab();
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

async function saveClipboardTab() {
  const on = /** @type {HTMLInputElement} */ ($("pref-clipboard-tab")).checked;
  try {
    await invoke("set_clipboard_tab", { clipboardTab: on });
    state.clipboardTab = on;
    if (!on) closeLightbox();
    applyTab();
    fit();
  } catch (e) {
    state.notice = { tone: "err", text: String(e) };
    render();
    refreshPrefs();
  }
}

async function saveOutboxLimit() {
  const input = /** @type {HTMLInputElement} */ ($("pref-outbox-limit"));
  try {
    const n = await invoke("set_clipboard_outbox_limit", { limit: Number(input.value) });
    input.value = String(n);
    refreshClipboard();
  } catch (e) {
    state.notice = { tone: "err", text: String(e) };
    render();
    refreshPrefs();
  }
}

async function refreshClipboard() {
  try {
    state.outbox = await invoke("clipboard_outbox");
    if (state.expandedSeq != null && !state.outbox.some((i) => i.seq === state.expandedSeq)) {
      state.expandedSeq = null;
    }
    renderClipboard();
    fit();
  } catch (e) {
    state.notice = { tone: "err", text: String(e) };
    render();
  }
}

function renderClipboard() {
  const list = $("clipboard-list");
  list.replaceChildren();
  if (!state.outbox.length) {
    list.append(h("p", { class: "empty" }, "没有未发送的内容"));
    return;
  }
  for (const item of state.outbox) {
    const expanded = state.expandedSeq === item.seq;
    const thumb = item.png_base64
      ? h("img", {
          class: "clip-thumb",
          src: `data:image/png;base64,${item.png_base64}`,
          alt: "",
          onclick: (/** @type {MouseEvent} */ e) => {
            e.stopPropagation();
            openLightbox(`data:image/png;base64,${item.png_base64}`);
          },
        })
      : null;
    list.append(
      h(
        "div",
        {
          class: `clip-item${expanded ? " expanded" : ""}`,
          onclick: () => {
            if (item.text == null) return;
            state.expandedSeq = expanded ? null : item.seq;
            renderClipboard();
            fit();
          },
        },
        h(
          "div",
          { class: "meta" },
          item.peer ? h("span", { class: "peer" }, item.peer) : null,
          h("div", { class: "clip-text" }, item.text ?? "图片"),
        ),
        thumb,
      ),
    );
  }
}

function openLightbox(src) {
  const img = /** @type {HTMLImageElement} */ ($("clipboard-lightbox-img"));
  img.src = src;
  $("clipboard-lightbox").hidden = false;
}

function closeLightbox() {
  $("clipboard-lightbox").hidden = true;
  /** @type {HTMLImageElement} */ ($("clipboard-lightbox-img")).src = "";
}

function lightboxOpen() {
  return !$("clipboard-lightbox").hidden;
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
    const removing = state.removing === peer.fingerprint;
    list.append(
      h(
        "div",
        { class: "device" + (online ? " online" : "") },
        h("i", { class: "dot" }),
        h("div", { class: "who" }, h("b", {}, peer.name), h("small", {}, sub)),
        h("span", { class: "state" }, online ? "在线" : "离线"),
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
        title: [c.name, platform, c.port ?? "接口未识别"].filter(Boolean).join(" · "),
        onclick: () => handOffTo(c),
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
  $("serial").textContent = d?.serial ? `SN ${d.serial}` : "";
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
function fit() {
  cancelAnimationFrame(fitFrame);
  fitFrame = requestAnimationFrame(() => {
    const panel = $("panel").getBoundingClientRect();
    const menu = $("menu");
    const bottom = menu.hidden ? panel.bottom : Math.max(panel.bottom, menu.getBoundingClientRect().bottom);
    invoke("fit_panel", { height: Math.ceil(bottom) }).catch(() => {});
  });
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
  const tabs = visibleTabs();
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
});

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
    if (lightboxOpen()) {
      closeLightbox();
      return;
    }
    closeMenu();
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
$("pref-clipboard-tab").addEventListener("change", saveClipboardTab);
$("pref-outbox-limit").addEventListener("change", saveOutboxLimit);
$("pref-autostart").addEventListener("change", saveAutostart);
$("clipboard-lightbox").addEventListener("click", closeLightbox);
window.addEventListener("blur", () => {
  if (recordingHotkey) cancelHotkeyRecord();
});

tauri.event.listen("panel-shown", () => {
  closeMenu();
  reveal();
  if (state.pending == null) refresh();
  refreshDevices();
  refreshPrefs();
  refreshClipboard();
});

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

tauri.event.listen("clipboard-changed", () => {
  refreshClipboard();
});

tauri.event.listen("notice", (/** @type {{ payload: string }} */ e) => {
  state.notice = { tone: "idle", text: e.payload };
  render();
});

invoke("agent_status")
  .then((/** @type {AgentStatus} */ status) => {
    state.agent = status;
    render();
  })
  .catch(() => {});

refresh();
refreshDevices();
refreshPrefs();
refreshClipboard();
