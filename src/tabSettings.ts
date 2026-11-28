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
  /** A4：退出时彻底关闭所有标签（默认 false＝保留当前状态下次恢复）。 */
  clearTabsOnExit: boolean;
}

const SETTINGS_KEY = "huaxia.tabs.settings";
const STATE_KEY = "huaxia.tabs.state";

/** 设置变更事件：设置面板写入后派发，标签壳监听重读。 */
export const TAB_SETTINGS_EVENT = "huaxia:tabs-settings";

const DEFAULTS: TabSettings = {
  newBehavior: "new",
  newTabPage: "huaxia",
  // A4：默认恢复上次的标签；用户可在设置里改成「启动时打开主页」。
  startupMode: "last",
  startupKind: "collection",
  startupId: null,
  masked: false,
  clearTabsOnExit: false,
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
        parsed.startupMode === "home" || parsed.startupMode === "specific"
          ? parsed.startupMode
          : "last",
      startupKind:
        parsed.startupKind === "tag" || parsed.startupKind === "loose"
          ? parsed.startupKind
          : "collection",
      startupId:
        typeof parsed.startupId === "number" && Number.isFinite(parsed.startupId)
          ? parsed.startupId
          : null,
      masked: parsed.masked === true,
      clearTabsOnExit: parsed.clearTabsOnExit === true,
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

/** 标签组（M5 Edge 化）：按 kind 归类，可折叠。组归属按 kind 存，重启后稳定。 */
export interface TabGroup {
  id: string;
  title: string;
  /** 组色（CSS 颜色串），画在组头小圆点上。 */
  color: string;
  collapsed: boolean;
}

/** A4：完整标签快照（含 browser 的 url、gallery 的 archiveId），下次启动原样恢复。 */
export interface PersistedWorkTab {
  id: string;
  kind: string;
  title: string;
  url?: string | null;
  archiveId?: number;
}

/** 上次关闭时的标签集合。kinds 是老结构（只存 kind），tabs 是 A4 起的完整快照。 */
export interface PersistedTabs {
  kinds: string[];
  active: string;
  /** M5：标签组定义；老数据没有此字段，读时补空数组。 */
  groups?: TabGroup[];
  /** M5：kind → groupId 的归属映射。 */
  groupOf?: Record<string, string>;
  /** A4：完整标签快照；老数据没有此字段时回退 kinds 路径。 */
  tabs?: PersistedWorkTab[];
}

/** 内置的默认分组方案：画匣功能页 / 阅读 / 工具，浏览器单独一组。 */
export const DEFAULT_TAB_GROUPS: TabGroup[] = [
  { id: "grp-core", title: "画匣", color: "#7c93ff", collapsed: false },
  { id: "grp-read", title: "阅读", color: "#e8a35c", collapsed: false },
  { id: "grp-web", title: "网页", color: "#5cc8a0", collapsed: false },
];

/** 默认归属：home/tags/trash/release/settings/net → 画匣；gallery → 阅读；browser → 网页。 */
export function defaultGroupOf(kind: string): string {
  if (kind === "gallery") return "grp-read";
  if (kind === "browser") return "grp-web";
  return "grp-core";
}

export function readPersistedTabs(): PersistedTabs | null {
  try {
    const raw = window.localStorage.getItem(STATE_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as Partial<PersistedTabs>;
    if (!Array.isArray(parsed.kinds)) return null;
    const groups = Array.isArray(parsed.groups)
      ? parsed.groups.filter(
          (g): g is TabGroup =>
            !!g &&
            typeof g.id === "string" &&
            typeof g.title === "string" &&
            typeof g.color === "string",
        ).map((g) => ({ ...g, collapsed: g.collapsed === true }))
      : undefined;
    const groupOf =
      parsed.groupOf && typeof parsed.groupOf === "object"
        ? Object.fromEntries(
            Object.entries(parsed.groupOf).filter(
              (entry): entry is [string, string] => typeof entry[1] === "string",
            ),
          )
        : undefined;
    // A4：完整标签快照，逐字段校验；老数据没有此字段时回退 kinds 路径。
    const tabs = Array.isArray(parsed.tabs)
      ? parsed.tabs.filter(
          (t): t is PersistedWorkTab =>
            !!t &&
            typeof t.id === "string" &&
            typeof t.kind === "string" &&
            typeof t.title === "string" &&
            (t.url === undefined || t.url === null || typeof t.url === "string") &&
            (t.archiveId === undefined ||
              (typeof t.archiveId === "number" && Number.isFinite(t.archiveId))),
        )
      : undefined;
    return {
      kinds: parsed.kinds.filter((kind): kind is string => typeof kind === "string"),
      active: typeof parsed.active === "string" ? parsed.active : "home",
      groups,
      groupOf,
      tabs,
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
