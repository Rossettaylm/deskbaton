// @ts-check

/**
 * @typedef {{ width: number, height: number, pixel_width: number, pixel_height: number,
 *             refresh_hz: number | null, hidpi: boolean }} ScreenView
 * @typedef {{ label: string, code: number, kind: "dp" | "hdmi" | "usb_c" | "other",
 *             active: boolean }} InputButton
 * @typedef {{ id: string, name: string, serial: string | null, subtitle: string,
 *             screen: ScreenView | null, current_input: number | null,
 *             current_label: string | null, inputs: InputButton[], preferred: boolean }} DisplayView
 * @typedef {{ displays: DisplayView[], config_error: string | null }} Snapshot
 * @typedef {{ kind: "confirmed", code: number, label: string } | { kind: "left_local", label: string }} SwitchOutcome
 */

const tauri = /** @type {any} */ (window).__TAURI__;
/** @type {<T>(cmd: string, args?: object) => Promise<T>} */
const invoke = tauri.core.invoke;

const SELECTED_KEY = "deskbaton.display";

const PORT_PATHS = {
  dp: '<path d="M2 2h26v7l-4 3H2z"/>',
  hdmi: '<path d="M2 2h26v6l-3 4H5l-3-4z"/>',
  usb_c: '<rect x="4" y="2.5" width="22" height="9" rx="4.5"/>',
  other: '<rect x="3" y="2.5" width="24" height="9" rx="1.5"/>',
};

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
      state.notice = { tone: "idle", text: `已切到 ${outcome.label}，本机不再持有画面` };
    }
  } catch (e) {
    state.pending = null;
    state.notice = { tone: "err", text: String(e) };
  }
  render();
}

function render() {
  const d = selected();
  renderHead(d);
  renderScreen(d);
  renderPorts(d);
  renderFoot(d);
  renderMenu();
  fit();
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
    if (screen.refresh_hz != null) parts.push(`${screen.refresh_hz} Hz`);
    sub.replaceChildren(...parts.map((p) => h("span", {}, p)));
  } else {
    res.textContent = d ? "未知分辨率" : state.loading ? "读取中" : "无显示器";
    sub.textContent = "";
  }

  const away = !!d && d.current_input == null;
  bezel.classList.toggle("dark", !d || away);
  $("now").classList.toggle("away", !d || away);
  $("now-label").textContent = d && !away ? (d.current_label ?? "—") : d ? "其它输入" : "—";
}

/** @param {DisplayView | null} d */
function renderPorts(d) {
  const ports = $("ports");
  ports.replaceChildren();
  if (!d) {
    ports.append(h("div", { class: "empty" }, state.loading ? "正在读取…" : "没有可切换的显示器"));
    return;
  }
  const leftTo = state.leftTo.get(d.id);
  for (const input of d.inputs) {
    const pending = state.pending === input.code;
    const switched = !input.active && leftTo === input.code;
    const button = h(
      "button",
      {
        class: ["port", input.active || switched ? "on" : "", pending ? "pending" : ""].join(" "),
        disabled: state.pending != null,
        title: `0x${input.code.toString(16).toUpperCase().padStart(2, "0")}`,
        onclick: () => switchTo(d, input),
      },
      h("i", { class: "led" }),
      switched ? h("span", { class: "tag" }, "已切到") : null,
    );
    button.insertAdjacentHTML("beforeend", `<svg viewBox="0 0 30 14">${PORT_PATHS[input.kind]}</svg>`);
    button.append(input.label);
    ports.append(button);
  }
}

/** @param {DisplayView | null} d */
function renderFoot(d) {
  const status = $("status");
  let tone = "idle";
  let text;
  if (state.notice) {
    ({ tone, text } = state.notice);
  } else if (state.loading) {
    text = "正在读取…";
  } else if (state.configError) {
    tone = "err";
    text = state.configError;
  } else if (!d) {
    text = "未发现显示器";
  } else if (d.current_input != null) {
    tone = "ok";
    text = "DDC 正常";
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
  if (e.key === "Escape") closeMenu();
});

tauri.event.listen("panel-shown", () => {
  closeMenu();
  reveal();
  if (state.pending == null) refresh();
});

refresh();
