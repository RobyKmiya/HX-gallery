/**
 * 标签范式设置（管理员 2026-11-25）。
 *
 * 三项集中在 localStorage，标签壳与设置面板共用同一份读写与变更事件：
 * - newBehavior：开新页面时「新建标签 / 复用原标签」；
 * - startupMode：启动时打开「主页 / 上次的标签 / 指定收藏」；
 * - masked：启动遮罩（--masked 的雏形，先做本地开关，壁纸锁接上后再联动）。
 *
 * 同标签页的 storage 事件不触发，所以改完手动派发自定义事件通知壳重读。
 */
export type NewTabBehavior = "new" | "reuse";
/** 浏览器新标签打开什么：画匣自己的新页（默认），或直接 Bing。 */
export type NewTabPage = "huaxia" | "bing";
export type StartupMode = "home" | "last" | "specific";
export type StartupTargetKind = "collection" | "tag" | "loose";

export interface StartupTarget {
  kind: StartupTargetKind;
  id: number | null;
}

export interface TabSettings {
  newBehavior: NewTabBehavior;
  /** 新标签显示画匣新页还是 Bing；默认画匣新页。 */
  newTabPage: NewTabPage;
  startupMode: StartupMode;
  startupKind: StartupTargetKind;
  startupId: number | null;
  masked: boolean;
}

const SETTINGS_KEY = "huaxia.tabs.settings";
const STATE_KEY = "huaxia.tabs.state";

/** 设置变更事件：设置面板写入后派发，标签壳监听重读。 */
export const TAB_SETTINGS_EVENT = "huaxia:tabs-settings";

const DEFAULTS: TabSettings = {
  newBehavior: "new",
  newTabPage: "huaxia",
  startupMode: "home",
  startupKind: "collection",
  startupId: null,
  masked: false,
};

export function readTabSettings(): TabSettings {
  try {
    const raw = window.localStorage.getItem(SETTINGS_KEY);
    if (!raw) return { ...DEFAULTS };
    const parsed = JSON.parse(raw) as Partial<TabSettings>;
    return {
      newBehavior: parsed.newBehavior === "reuse" ? "reuse" : "new",
      newTabPage: parsed.newTabPage === "bing" ? "bing" : "huaxia",
      startupMode:
        parsed.startupMode === "last" || parsed.startupMode === "specific"
          ? parsed.startupMode
          : "home",
      startupKind:
        parsed.startupKind === "tag" || parsed.startupKind === "loose"
          ? parsed.startupKind
          : "collection",
      startupId:
        typeof parsed.startupId === "number" && Number.isFinite(parsed.startupId)
          ? parsed.startupId
          : null,
      masked: parsed.masked === true,
    };
  } catch {
    return { ...DEFAULTS };
  }
}

export function saveTabSettings(next: TabSettings): void {
  try {
    window.localStorage.setItem(SETTINGS_KEY, JSON.stringify(next));
  } catch {
    // 存不了就只影响本次，不影响功能
  }
  window.dispatchEvent(new CustomEvent(TAB_SETTINGS_EVENT));
}

/** 启动时要应用的目标（仅 startupMode = specific 时有意义）。 */
export function readStartupTarget(settings: TabSettings): StartupTarget | null {
  if (settings.startupMode !== "specific") return null;
  return { kind: settings.startupKind, id: settings.startupId };
}

/** 上次关闭时的标签集合（只存非浏览器标签，浏览器恢复会重开 webview，太慢）。 */
export interface PersistedTabs {
  kinds: string[];
  active: string;
}

export function readPersistedTabs(): PersistedTabs | null {
  try {
    const raw = window.localStorage.getItem(STATE_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as Partial<PersistedTabs>;
    if (!Array.isArray(parsed.kinds)) return null;
    return {
      kinds: parsed.kinds.filter((kind): kind is string => typeof kind === "string"),
      active: typeof parsed.active === "string" ? parsed.active : "home",
    };
  } catch {
    return null;
  }
}

export function savePersistedTabs(state: PersistedTabs): void {
  try {
    window.localStorage.setItem(STATE_KEY, JSON.stringify(state));
  } catch {
    // 忽略
  }
}
