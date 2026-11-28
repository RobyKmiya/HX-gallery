/**
 * 工作台多标签（P1，方案见 docs/浏览器内置阅读器方案.md）。
 *
 * 形态：顶部标签条。主页与「标签中心 / 回收站 / 发版 / 设置 / 链接下载」
 * 都是标签，共用同一个 LibraryView 实例（按 activeKind 内部分发渲染），
 * 浏览器标签：url 为空时显示 React 新页（NewTab），有 url 才挂原生子 webview。
 *
 * 关键规则：
 * - 原生子 webview 是独立 HWND，React 模态盖不住 → 任何模态打开时必须
 *   notifyModalOpen() 让浏览器 host 整体隐藏，关掉再恢复；
 * - 引擎切标签只隐藏不销毁，回来原样（登录态/滚动都在）；
 * - 关浏览器标签 = d2_embed_close，绝不碰宿主窗；
 * - 非浏览器标签共享一个 LibraryView，切标签只换 activeKind，状态不丢。
 */
import { useEffect, useRef, useState } from "react";
import { usePointerDrag } from "../usePointerDrag";
import { emitTo, listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { api } from "../api";
import type { LibraryState, NavIntent } from "../types";
import LibraryView from "./LibraryView";
import NewTab from "./NewTab";
import GalleryTab from "./GalleryTab";
import {
  DEFAULT_TAB_GROUPS,
  TAB_SETTINGS_EVENT,
  defaultGroupOf,
  readPersistedTabs,
  readStartupTarget,
  readTabSettings,
  savePersistedTabs,
  type StartupTarget,
  type TabGroup,
  type TabSettings,
} from "../tabSettings";
import { EVENT_MODAL } from "../modalBus";

export interface WorkTab {
  id: string;
  kind: TabKind;
  title: string;
  /** 仅浏览器标签用：null/undefined = 新页，有值 = 加载该网址的 webview。 */
  url?: string | null;
  /** 仅 gallery 标签用：要打开的画集 id。 */
  archiveId?: number;
}

export type TabKind =
  | "home"
  | "gallery"
  | "browser"
  | "tags"
  | "trash"
  | "release"
  | "settings"
  | "net";

/** 单例标签：已存在时再开就召回，不重复开。其余（browser / net）可多开。 */
const SINGLETON: TabKind[] = ["home", "tags", "trash", "release", "settings"];

const TAB_GLYPH: Record<TabKind, string> = {
  home: "🏠",
  gallery: "📖",
  browser: "🌐",
  tags: "#️⃣",
  trash: "🗑",
  release: "🚀",
  settings: "⚙️",
  net: "⬇️",
};

const TAB_TITLE: Record<TabKind, string> = {
  home: "主页",
  gallery: "阅读",
  browser: "新页",
  tags: "标签中心",
  trash: "回收站",
  release: "发版",
  settings: "设置",
  net: "链接下载",
};

const KNOWN_KINDS = Object.keys(TAB_TITLE) as TabKind[];

function isTabKind(value: string): value is TabKind {
  return (KNOWN_KINDS as string[]).includes(value);
}

function hostOf(url: string): string {
  try {
    return new URL(url).hostname.replace(/^www\./, "");
  } catch {
    return url;
  }
}

/** M5③：拆窗数据在主窗写入 localStorage，新窗按 token 读回（避免中文标题走 URL 编码失真）。 */
export const DETACH_KEY_PREFIX = "huaxia.detached.";
/** initialTabs() 与 initialActiveId() 会各调一次，token 只能消费一遍，故缓存。 */
let detachedTabCache: { done: true; tab: WorkTab | null } | null = null;

function readDetachedTab(): WorkTab | null {
  if (detachedTabCache) return detachedTabCache.tab;
  let result: WorkTab | null = null;
  try {
    const token = new URLSearchParams(window.location.search).get("detach");
    if (!token) return null;
    const raw = window.localStorage.getItem(`${DETACH_KEY_PREFIX}${token}`);
    if (!raw) return null;
    const tab = JSON.parse(raw) as Partial<WorkTab>;
    window.localStorage.removeItem(`${DETACH_KEY_PREFIX}${token}`);
    if (!tab.id || typeof tab.kind !== "string" || !isTabKind(tab.kind)) return null;
    result = {
      id: tab.id,
      kind: tab.kind,
      title: tab.title ?? TAB_TITLE[tab.kind],
      url: tab.url ?? null,
      archiveId: typeof tab.archiveId === "number" ? tab.archiveId : undefined,
    };
  } catch {
    result = null;
  } finally {
    detachedTabCache = { done: true, tab: result };
  }
  return result;
}

/** 启动时决定初始标签集合：拆窗标签 / 主页 / 上次的完整标签快照 / 指定收藏。 */
function initialTabs(): WorkTab[] {
  const detached = readDetachedTab();
  if (detached) return [detached];
  const settings = readTabSettings();
  if (settings.startupMode === "last") {
    const persisted = readPersistedTabs();
    if (persisted) {
      // A4：优先完整快照（含 browser 的 url / gallery 的 archiveId）。
      if (persisted.tabs?.length) {
        const restored = persisted.tabs
          .filter((tab) => isTabKind(tab.kind))
          .filter(
            (tab) =>
              // gallery 必须有合法 archiveId；browser 允许空 url（新页）。
              (tab.kind !== "gallery" ||
                (typeof tab.archiveId === "number" && tab.archiveId > 0)),
          )
          .slice(0, 12)
          .map((tab) => ({
            id: tab.id,
            kind: tab.kind as TabKind,
            title: tab.title || TAB_TITLE[tab.kind as TabKind],
            url: tab.url ?? null,
            archiveId: tab.archiveId,
          }));
        if (restored.length) {
          // 保证主页总在（home 标签不可关）。
          if (!restored.some((tab) => tab.kind === "home")) {
            restored.unshift({
              id: "home",
              kind: "home",
              title: TAB_TITLE.home,
              url: null,
              archiveId: undefined,
            });
          }
          return restored;
        }
      }
      // 老数据兜底：只有 kinds 字符串数组。
      const kinds = persisted.kinds
        .filter(
          (kind): kind is TabKind =>
            isTabKind(kind) && kind !== "browser" && kind !== "gallery",
        )
        .filter((kind, index, list) => kind === "home" || list.indexOf(kind) === index);
      const ordered = (["home"] as TabKind[])
        .concat(kinds.filter((kind) => kind !== "home"))
        .slice(0, 8);
      return ordered.map((kind) => ({
        id: kind,
        kind,
        title: TAB_TITLE[kind],
      }));
    }
  }
  return [{ id: "home", kind: "home", title: TAB_TITLE.home }];
}

function initialActiveId(tabs: WorkTab[]): string {
  if (new URLSearchParams(window.location.search).get("detach")) {
    return tabs[0]?.id ?? "home";
  }
  const settings = readTabSettings();
  if (settings.startupMode === "last") {
    const persisted = readPersistedTabs();
    if (persisted) {
      // 新快照存的是 tab.id；老数据存的是 kind，两种都兼容。
      const byId = tabs.find((tab) => tab.id === persisted.active);
      if (byId) return byId.id;
      const byKind = tabs.find((tab) => tab.kind === persisted.active);
      if (byKind) return byKind.id;
    }
  }
  return tabs[0]?.id ?? "home";
}

function BrowserHost({
  url,
  visible,
  home,
  onHome,
  onClosed,
}: {
  url: string;
  visible: boolean;
  /** 工具栏 ⌂ 指向：huaxia:newtab（回画匣新页）或 bing 网址。 */
  home: string;
  onHome: () => void;
  /** 引擎把内嵌浏览器整个拆掉时（Ctrl+W 关最后一个标签），壳把这个标签一并关。 */
  onClosed: () => void;
}) {
  const holderRef = useRef<HTMLDivElement>(null);
  const [label, setLabel] = useState<string | null>(null);
  const modalCount = useRef(0);
  const [modalTick, setModalTick] = useState(0);
  /** onHome/onClosed 每次渲染都是新函数，用 ref 挡住重复订阅。 */
  const onHomeRef = useRef(onHome);
  onHomeRef.current = onHome;
  const onClosedRef = useRef(onClosed);
  onClosedRef.current = onClosed;

  // 引擎生命周期：挂载/换址时开，卸载时拆（切标签不卸载，只切 visible）。
  useEffect(() => {
    let dead = false;
    let mine: string | null = null;
    setLabel(null);
    void (async () => {
      try {
        // home 按设置走：默认 huaxia:newtab（⌂ 发事件 → 主窗切回新页），
        // 而不是把 e-hentai 当成主页（管理员 2026-11-25 反馈的小房子问题）。
        // 先量占位矩形再开引擎：Rust 从第一帧就把工具栏摆到位。
        // 以前不传，Rust 用 (0,0)+900px 建工具栏，首次 embed_sync 又必失败，
        // 顶栏会错位并列 ~800ms（管理员 2026-11-26 截图）。
        const rectEl = holderRef.current;
        const rect = rectEl?.getBoundingClientRect();
        // A7：宿主写死 main 会让拆出窗的引擎挂到主窗上；显式传当前窗 label。
        const hostLabel = getCurrentWindow().label;
        const opened = await api.d2EmbedOpen({
          url,
          adblock: true,
          home,
          hostLabel,
          rect: rect
            ? { x: rect.left, y: rect.top, w: rect.width, h: rect.height }
            : undefined,
        });
        if (dead) {
          void api.d2EmbedClose(opened).catch(() => {});
          return;
        }
        mine = opened;
        setLabel(opened);
      } catch {
        // 引擎开不了就留白（P2 会把失败提示接进标签条）
      }
    })();
    return () => {
      dead = true;
      if (mine) void api.d2EmbedClose(mine).catch(() => {});
    };
  }, [url]);

  // 工具栏 ⌂ → 主窗把该标签切回「画匣新页」（卸载 webview，由 React 接管）。
  useEffect(() => {
    if (!label) return;
    let dead = false;
    let unlisten: (() => void) | null = null;
    void listen<string>("huaxia://newtab", (event) => {
      if (event.payload === label) onHomeRef.current();
    }).then((un) => {
      if (dead) un();
      else unlisten = un;
    });
    return () => {
      dead = true;
      if (unlisten) unlisten();
    };
  }, [label]);

  // 引擎把内嵌浏览器整个拆掉（Ctrl+W 关掉最后一个内容标签）→ 壳把这个
  // 网页标签一并关掉，否则标签还挂着、内容区只剩一块黑（1.1.96）。
  // 注意自己拆（onClose / ⌂）也会走到 embed_close：closeTab 对已移除的
  // 标签是空操作，重复触发无害。
  useEffect(() => {
    if (!label) return;
    let dead = false;
    let unlisten: (() => void) | null = null;
    void listen<string>("huaxia://browser-closed", (event) => {
      if (event.payload === label) onClosedRef.current();
    }).then((un) => {
      if (dead) un();
      else unlisten = un;
    });
    return () => {
      dead = true;
      if (unlisten) unlisten();
    };
  }, [label]);

  // 摆位：ResizeObserver + resize + 低频兜底（布局抖动不丢跟随）。
  useEffect(() => {
    const el = holderRef.current;
    if (!el) return;
    const sync = () => {
      if (!label) return;
      const rect = el.getBoundingClientRect();
      // rect 是 CSS 像素 = 逻辑像素，直接给 Rust（Tauri 自己乘 scale）。
      void api
        .d2EmbedSync({ label, x: rect.left, y: rect.top, w: rect.width, h: rect.height })
        .catch(() => {});
    };
    sync();
    const observer = new ResizeObserver(sync);
    observer.observe(el);
    window.addEventListener("resize", sync);
    const timer = window.setInterval(sync, 800);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", sync);
      window.clearInterval(timer);
    };
  }, [label]);

  // 显隐 = 标签激活 且 没有模态挡着。
  useEffect(() => {
    if (!label) return;
    const visibleNow = visible && modalCount.current === 0;
    void api.d2EmbedShow({ label, visible: visibleNow }).catch(() => {});
  }, [label, visible, modalTick]);

  // 模态事件 → 隐藏/恢复浏览器。
  useEffect(() => {
    const onModal = (event: Event) => {
      const open = (event as CustomEvent).detail?.open === true;
      modalCount.current = Math.max(0, modalCount.current + (open ? 1 : -1));
      setModalTick((tick) => tick + 1);
    };
    window.addEventListener(EVENT_MODAL, onModal);
    return () => window.removeEventListener(EVENT_MODAL, onModal);
  }, []);

  return (
    <div
      ref={holderRef}
      className="browser-host"
      style={{
        position: "absolute",
        inset: 0,
        visibility: visible && label ? "visible" : "hidden",
      }}
    />
  );
}

export default function WorkspaceTabs({
  library,
  onLibraryChanged,
  onSwitchLibrary,
}: {
  library: LibraryState;
  onLibraryChanged: () => Promise<LibraryState>;
  onSwitchLibrary: () => void;
}) {
  const [settings, setSettings] = useState<TabSettings>(() => readTabSettings());
  const [tabs, setTabs] = useState<WorkTab[]>(() => initialTabs());
  const [activeId, setActiveId] = useState(() => initialActiveId(initialTabs()));
  // M5①：标签分组。从持久化的 groupOf/groups 恢复，老数据走默认方案。
  const [groups, setGroups] = useState<TabGroup[]>(() => {
    const saved = readPersistedTabs();
    return saved?.groups?.length ? saved.groups : DEFAULT_TAB_GROUPS.map((g) => ({ ...g }));
  });
  const [groupOf, setGroupOf] = useState<Record<string, string>>(() => {
    const saved = readPersistedTabs();
    return saved?.groupOf ?? {};
  });
  /** 标签右键菜单：定位 + 目标 tab。 */
  const [tabMenu, setTabMenu] = useState<{ id: string; x: number; y: number } | null>(null);
  /** A4/A7：当前是否为拆出的独立窗（带 ?detach=token）。 */
  const isDetachedWindow = !!new URLSearchParams(window.location.search).get("detach");
  /** 启动目标只在首帧算一次。 */
  const startupTargetRef = useRef<StartupTarget | null>(readStartupTarget(readTabSettings()));
  /** 启动遮罩：仅首帧按设置决定，之后点一下揭开。 */
  const [masked, setMasked] = useState(() => readTabSettings().masked);
  /** 新页发来的导航意图（真正筛选在 LibraryView 里做）。 */
  const [navRequest, setNavRequest] = useState<(NavIntent & { token: number }) | null>(null);
  const hydratedRef = useRef(false);

  const activeTab = tabs.find((tab) => tab.id === activeId) ?? tabs[0];
  const libraryVisible = activeTab?.kind !== "browser";
  /** LibraryView 内部据此分发：browser 激活时它被隐藏，给个 home 占位即可。 */
  const reactKind = activeTab && activeTab.kind !== "browser" ? activeTab.kind : "home";
  const browserCount = tabs.filter((tab) => tab.kind === "browser").length;

  // 设置面板改完 → 壳重读（新开标签行为 / 启动页立即对后续生效）。
  useEffect(() => {
    const onSettings = () => setSettings(readTabSettings());
    window.addEventListener(TAB_SETTINGS_EVENT, onSettings);
    return () => window.removeEventListener(TAB_SETTINGS_EVENT, onSettings);
  }, []);

  // A7：独立窗「合并回主窗」→ 主窗读 token 还原标签（不跨窗拖拽的替代方案）。
  useEffect(() => {
    if (isDetachedWindow) return;
    let dead = false;
    let unlisten: (() => void) | null = null;
    void listen<string>("huaxia://reattach-tab", (event) => {
      const token = event.payload;
      try {
        const raw = window.localStorage.getItem(`${DETACH_KEY_PREFIX}${token}`);
        window.localStorage.removeItem(`${DETACH_KEY_PREFIX}${token}`);
        if (!raw) return;
        const tab = JSON.parse(raw) as Partial<WorkTab>;
        if (!tab.kind || !isTabKind(tab.kind)) return;
        if (tab.kind === "browser" && tab.url) {
          openUrlTab(tab.url);
        } else {
          openTab(tab.kind, tab.title, true, tab.archiveId);
        }
      } catch {
        // 合并失败就忽略，用户还能从新窗重新操作。
      }
    }).then((un) => {
      if (dead) un();
      else unlisten = un;
    });
    return () => {
      dead = true;
      if (unlisten) unlisten();
    };
    // openTab/openUrlTab 用 ref 里的 tabs/settings，这里只需订阅一次。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isDetachedWindow]);

  // 标签集合落盘（跳过首帧，避免用默认主页覆盖「上次的标签」）。
  // A4：写完整快照；「退出时彻底关闭」开关打开时写空态；拆出的新窗不覆盖主窗快照。
  useEffect(() => {
    if (!hydratedRef.current) {
      hydratedRef.current = true;
      return;
    }
    if (isDetachedWindow) return; // 新窗标签独立，绝不写回共享的持久化快照。
    const clear = settings.clearTabsOnExit;
    savePersistedTabs({
      kinds: clear ? [] : tabs.map((tab) => tab.kind),
      active: clear ? "home" : activeId,
      groups,
      groupOf,
      tabs: clear
        ? []
        : tabs.map((tab) => ({
            id: tab.id,
            kind: tab.kind,
            title: tab.title,
            url: tab.url ?? null,
            archiveId: tab.archiveId,
          })),
    });
  }, [tabs, activeId, groups, groupOf, settings.clearTabsOnExit]);

  /** 统一开标签入口：单例已存在则召回；可多开的按「新开页行为」决定新建还是复用。 */
  const openTab = (
    kind: TabKind,
    title?: string,
    forceNew = false,
    archiveId?: number,
  ) => {
    // gallery 标签永远新开：每本画集要独立 Reader 实例，不能被「复用」合并。
    const replaceable =
      kind !== "gallery" &&
      (SINGLETON.includes(kind) || (!forceNew && settings.newBehavior === "reuse"));
    if (!forceNew && replaceable) {
      const existing = tabs.find((tab) => tab.kind === kind);
      if (existing) {
        setActiveId(existing.id);
        return existing.id;
      }
    }
    const id = `${kind}-${Date.now().toString(36)}${Math.random()
      .toString(36)
      .slice(2, 6)}`;
    const count = tabs.filter((tab) => tab.kind === kind).length;
    const name =
      title ?? (count > 0 ? `${TAB_TITLE[kind]} ${count + 1}` : TAB_TITLE[kind]);
    setTabs((prev) => [...prev, { id, kind, title: name, archiveId }]);
    setActiveId(id);
    return id;
  };

  /** 点开画集：新开一个 gallery 标签，主页原样保留。 */
  const openGallery = (card: { id: number; title: string }) => {
    openTab("gallery", card.title || `画集 ${card.id}`, true, card.id);
  };

  const addBrowserTab = () => {
    const id = `browser-${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
    // 「新标签打开」设为 Bing 就直接开 bing，否则开画匣新页（默认）。
    const bing = settings.newTabPage === "bing";
    const title = bing
      ? "bing.com"
      : browserCount === 0
        ? "新页"
        : `新页 ${browserCount + 1}`;
    setTabs((prev) => [
      ...prev,
      { id, kind: "browser", title, url: bing ? "https://www.bing.com/" : null },
    ]);
    setActiveId(id);
  };

  /** 详情页「源链接」等外部请求：直接开一个带地址的网页标签。 */
  const openUrlTab = (url: string) => {
    const id = `browser-${Date.now().toString(36)}${Math.random()
      .toString(36)
      .slice(2, 6)}`;
    setTabs((prev) => [
      ...prev,
      { id, kind: "browser", title: hostOf(url), url },
    ]);
    setActiveId(id);
  };

  // 引擎 Ctrl+T / 弹窗 about:blank → 外壳开自己的「新页」标签（不用 URL）。
  // ref 保新：事件监听只挂一次，别把首帧的 addBrowserTab 闭包冻在里面。
  const addBrowserTabRef = useRef(addBrowserTab);
  addBrowserTabRef.current = addBrowserTab;
  useEffect(() => {
    const handler = (event: Event) => {
      const url = (event as CustomEvent<string>).detail;
      if (url === "huaxia:newtab" || url === "about:blank") {
        addBrowserTabRef.current();
        return;
      }
      if (typeof url === "string" && /^https?:\/\//.test(url)) openUrlTab(url);
    };
    window.addEventListener("huaxia:open-url-tab", handler);
    return () => window.removeEventListener("huaxia:open-url-tab", handler);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 引擎 Ctrl+Tab → 循环切换工作台标签（内嵌引擎里只有一个内容标签，
  // 切内层看不到变化；外壳标签才是用户感知的「标签页」。1.1.100）。
  const tabsRef = useRef(tabs);
  tabsRef.current = tabs;
  const activeIdRef = useRef(activeId);
  activeIdRef.current = activeId;
  useEffect(() => {
    let dead = false;
    let unlisten: (() => void) | null = null;
    void listen<number>("huaxia://cycle-tab", (event) => {
      const list = tabsRef.current;
      if (list.length < 2) return;
      const index = list.findIndex((tab) => tab.id === activeIdRef.current);
      if (index < 0) return;
      const step = event.payload > 0 ? 1 : -1;
      const next = list[(index + step + list.length) % list.length];
      if (next) setActiveId(next.id);
    }).then((un) => {
      if (dead) un();
      else unlisten = un;
    });
    return () => {
      dead = true;
      if (unlisten) unlisten();
    };
  }, []);

  /** 新页里输入网址 / 小房子回新页：url=null 时退回 React 新页。 */
  const setTabUrl = (id: string, url: string | null) => {
    setTabs((prev) =>
      prev.map((tab) =>
        tab.id === id
          ? { ...tab, url, title: url ? hostOf(url) : TAB_TITLE.browser }
          : tab,
      ),
    );
  };

  /** 新页点画匣入口：切回主页并投递筛选意图。 */
  const navigate = (intent: NavIntent) => {
    setNavRequest({ ...intent, token: Date.now() });
    openTab("home");
  };

  const closeTab = (id: string) => {
    if (id === "home") return;
    setTabs((prev) => {
      const next = prev.filter((tab) => tab.id !== id);
      if (activeId === id) setActiveId(next[next.length - 1]?.id ?? "home");
      return next;
    });
  };

  /** M5：某标签所属组 id（未显式指定或组已不存在则回退到 kind 默认分组）。 */
  const groupIdOf = (tab: WorkTab) => {
    const assigned = groupOf[tab.id] ?? defaultGroupOf(tab.kind);
    return groups.some((group) => group.id === assigned) ? assigned : defaultGroupOf(tab.kind);
  };

  /** 切换组的折叠态（组头点击）。 */
  const toggleGroup = (groupId: string) => {
    setGroups((prev) =>
      prev.map((group) =>
        group.id === groupId ? { ...group, collapsed: !group.collapsed } : group,
      ),
    );
  };

  /** 把标签移到目标标签所在位置；跨组时同时改归属（M5②）。 */
  const reorderTabs = (fromId: string, toId: string) => {
    if (fromId === toId) return;
    setTabs((prev) => {
      const from = prev.findIndex((tab) => tab.id === fromId);
      const to = prev.findIndex((tab) => tab.id === toId);
      if (from < 0 || to < 0 || from === to) return prev;
      const next = [...prev];
      const [moved] = next.splice(from, 1);
      next.splice(to, 0, moved);
      return next;
    });
    // 跨组拖动：源标签跟随目标标签的组。
    setGroupOf((prev) => {
      const target = tabs.find((tab) => tab.id === toId);
      if (!target) return prev;
      const targetGroup = groupIdOf(target);
      if (groupIdOf(tabs.find((tab) => tab.id === fromId)!) === targetGroup) return prev;
      return { ...prev, [fromId]: targetGroup };
    });
  };

  /** M5③：把标签拆到独立窗口（后端命令见 lib.rs detach_tab）。 */
  const detachTab = (id: string, _x: number, _y: number) => {
    const tab = tabs.find((entry) => entry.id === id);
    if (!tab || tab.kind === "home") return;

    // A7/A8/A10：所有标签（含网页）统一走完整 App 窗。
    // 网页标签不再开独立浏览器引擎窗（那套 UI 与主窗不一致、双系统框、登录态割裂）；
    // 新窗里的 BrowserHost 会用 hostLabel 把引擎挂到新窗自己身上，数据天然同源。
    const token = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`;
    try {
      window.localStorage.setItem(
        `${DETACH_KEY_PREFIX}${token}`,
        JSON.stringify(tab),
      );
    } catch {
      return;
    }
    void api.detachTab(token).then((opened) => {
      if (!opened) window.localStorage.removeItem(`${DETACH_KEY_PREFIX}${token}`);
      // 功能标签拆出后主窗保留原标签（两窗共享库，各看各的）。
    }).catch(() => {
      window.localStorage.removeItem(`${DETACH_KEY_PREFIX}${token}`);
    });
  };

  /** A7：把独立窗当前标签合并回主窗（token 经 localStorage 共享，主窗监听还原）。 */
  const mergeBackToMain = () => {
    if (!isDetachedWindow) return;
    const tab = activeTab;
    if (!tab) return;
    const token = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`;
    try {
      window.localStorage.setItem(`${DETACH_KEY_PREFIX}${token}`, JSON.stringify(tab));
      void emitTo("main", "huaxia://reattach-tab", token).then(() => {
        void getCurrentWindow().close();
      }).catch(() => {
        window.localStorage.removeItem(`${DETACH_KEY_PREFIX}${token}`);
      });
    } catch {
      // localStorage 不可用时放弃合并，不打断使用。
    }
  };

  // 独立窗 Ctrl+Shift+M 合并回主窗。
  useEffect(() => {
    if (!isDetachedWindow) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.ctrlKey && event.shiftKey && event.key.toLowerCase() === "m") {
        event.preventDefault();
        mergeBackToMain();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  /** 标签条拖拽：横向排序 + 跨组归属 + 拖出拆窗。 */
  const tabDrag = usePointerDrag({
    axis: "x",
    onDrop: reorderTabs,
    onDropOutside: detachTab,
    insideSelector: ".workspace-tabs",
  });

  /** 面板里的「跳到该标签」走这里；home 关不掉，直接切过去。 */
  const activateTab = (kind: string) => {
    if (!isTabKind(kind)) return;
    openTab(kind);
  };

  return (
    <div style={{ display: "flex", flexDirection: "column", height: "100vh" }}>
      <div
        className="workspace-tabs"
        style={{
          display: "flex",
          alignItems: "center",
          gap: 4,
          padding: "4px 8px 0",
          background: "var(--bg-elev, #1c1c22)",
          borderBottom: "1px solid var(--border, #2c2c34)",
          flex: "0 0 auto",
          overflowX: "auto",
        }}
      >
        {groups.flatMap((group) => {
          const members = tabs.filter((tab) => groupIdOf(tab) === group.id);
          if (members.length === 0) return [];
          const collapsed = group.collapsed;
          const head = (
            <div
              key={`head-${group.id}`}
              className={`workspace-tab-group-head${collapsed ? " is-collapsed" : ""}`}
              data-group-id={group.id}
              onClick={() => toggleGroup(group.id)}
              title={collapsed ? `展开「${group.title}」组` : `折叠「${group.title}」组`}
            >
              <span className="group-dot" style={{ background: group.color }} />
              <span>{group.title}</span>
              <span className="group-count">{members.length}</span>
              <span className="group-caret">{collapsed ? "▸" : "▾"}</span>
            </div>
          );
          if (collapsed) return [head];
          return [
            head,
            ...members.map((tab) => {
              const active = tab.id === activeId;
              const dragging = tabDrag.dragId === tab.id;
              const dragOver = tabDrag.overId === tab.id && !dragging;
              return (
                <div
                  key={tab.id}
                  data-drag-id={tab.id}
                  onPointerDown={(event) => tabDrag.begin(event, tab.id)}
                  onClick={() => {
                    if (tabDrag.shouldIgnoreClick()) return;
                    setActiveId(tab.id);
                  }}
                  onContextMenu={(event) => {
                    event.preventDefault();
                    setTabMenu({ id: tab.id, x: event.clientX, y: event.clientY });
                  }}
                  className={`workspace-tab${active ? " is-active" : ""}${
                    dragging ? " is-dragging" : ""
                  }${dragOver ? " is-drag-over" : ""}`}
                  style={{
                    display: "flex",
                    alignItems: "center",
                    gap: 6,
                    padding: "5px 10px",
                    borderRadius: "8px 8px 0 0",
                    cursor: "pointer",
                    fontSize: 12.5,
                    whiteSpace: "nowrap",
                    width: 148,
                    flex: "0 0 148px",
                    overflow: "hidden",
                    background: active ? "var(--bg, #14141a)" : "transparent",
                    color: active ? "var(--text, #e8e8ee)" : "var(--text-dim, #9a9aa4)",
                    borderTop: active
                      ? `2px solid ${group.color}`
                      : "2px solid transparent",
                  }}
                  title={tab.kind === "home" ? "库主目录（不可关闭）" : tab.title}
                >
                  <span style={{ flex: "0 0 auto" }}>{TAB_GLYPH[tab.kind]}</span>
                  <span
                    style={{
                      flex: "1 1 auto",
                      overflow: "hidden",
                      textOverflow: "ellipsis",
                      whiteSpace: "nowrap",
                      textAlign: "left",
                    }}
                  >
                    {tab.title}
                  </span>
                  {tab.kind !== "home" && (
                    <button
                      type="button"
                      onClick={(event) => {
                        event.stopPropagation();
                        closeTab(tab.id);
                      }}
                      style={{
                        border: "none",
                        background: "transparent",
                        color: "inherit",
                        cursor: "pointer",
                        padding: "0 2px",
                        lineHeight: 1,
                        fontSize: 12,
                      }}
                      title="关闭标签"
                    >
                      ✕
                    </button>
                  )}
                </div>
              );
            }),
          ];
        })}
        <button
          type="button"
          onClick={addBrowserTab}
          style={{
            border: "none",
            background: "transparent",
            color: "var(--text-dim, #9a9aa4)",
            cursor: "pointer",
            fontSize: 14,
            padding: "4px 8px",
          }}
          title="新建浏览器标签"
        >
          ＋
        </button>
        {/* A7：独立窗的「合并回主窗」入口（指针事件不跨 HWND，用按钮/快捷键替代拖拽） */}
        {isDetachedWindow && (
          <button
            type="button"
            onClick={mergeBackToMain}
            style={{
              border: "none",
              background: "transparent",
              color: "var(--accent, #7c93ff)",
              cursor: "pointer",
              fontSize: 12,
              padding: "4px 8px",
              whiteSpace: "nowrap",
            }}
            title="把这个标签合并回主窗口（Ctrl+Shift+M）"
          >
            ⤓ 合并回主窗
          </button>
        )}
        {/* 自定义窗条拖拽区 + 窗口控制（原生标题栏已永久去除） */}
        <div
          data-tauri-drag-region
          style={{ flex: "1 1 auto", height: "100%", minHeight: 24, cursor: "default" }}
        />
        <div
          style={{ display: "flex", alignItems: "center", gap: 2, flex: "0 0 auto" }}
          onDoubleClick={(event) => event.stopPropagation()}
        >
          {(
            [
              ["—", "最小化", () => void getCurrentWindow().minimize()],
              ["□", "最大化/还原", () => void getCurrentWindow().toggleMaximize()],
              ["✕", "关闭", () => void getCurrentWindow().close()],
            ] as [string, string, () => void][]
          ).map(([glyph, tip, act]) => (
            <button
              key={tip}
              type="button"
              title={tip}
              onClick={act}
              style={{
                border: "none",
                background: "transparent",
                color: "var(--text-dim, #9a9aa4)",
                cursor: "pointer",
                fontSize: 13,
                width: 34,
                height: 26,
                lineHeight: "26px",
                padding: 0,
              }}
              onMouseEnter={(e) => {
                e.currentTarget.style.background =
                  tip === "关闭" ? "#c4362f" : "var(--bg, #14141a)";
                e.currentTarget.style.color = "#fff";
              }}
              onMouseLeave={(e) => {
                e.currentTarget.style.background = "transparent";
                e.currentTarget.style.color = "var(--text-dim, #9a9aa4)";
              }}
            >
              {glyph}
            </button>
          ))}
        </div>
      </div>
      <div style={{ position: "relative", flex: "1 1 auto", minHeight: 0, overflow: "hidden" }}>
        {/* 非浏览器标签共用一份 LibraryView：切标签只换 activeKind，状态不丢。 */}
        <div
          style={{
            position: "absolute",
            inset: 0,
            visibility: libraryVisible ? "visible" : "hidden",
            // 不可见时连渲染一起停掉：主页有大量卡片 + 封面 + 动效，只 visibility
            // hidden 时它仍在后台布局/绘制，切到「新页」会明显卡（管理员 2026-11-27）。
            contentVisibility: libraryVisible ? "visible" : "hidden",
          }}
        >
          <LibraryView
            library={library}
            onLibraryChanged={onLibraryChanged}
            onSwitchLibrary={onSwitchLibrary}
            onOpenTab={(kind, title) => openTab(kind as TabKind, title)}
            activeKind={reactKind}
            activeTabId={activeTab?.id ?? "home"}
            onActivateTab={activateTab}
            startupTarget={startupTargetRef.current}
            navRequest={navRequest}
            onNavHandled={() => setNavRequest(null)}
            onOpenGallery={openGallery}
          />
        </div>
        {/* 浏览器标签：url 空 = React 新页；有 url = 原生子 webview。 */}
        {tabs
          .filter((tab) => tab.kind === "browser")
          .map((tab) => (
            <div
              key={tab.id}
              style={{
                position: "absolute",
                inset: 0,
                // 这一层里有原生子 webview 的占位矩形（靠真实布局量 rect），
                // 不能 content-visibility:hidden，否则切回来算不出尺寸。
                visibility: activeId === tab.id ? "visible" : "hidden",
              }}
            >
              {tab.url ? (
                <BrowserHost
                  url={tab.url}
                  visible={activeId === tab.id}
                  home={
                    settings.newTabPage === "bing"
                      ? "https://www.bing.com/"
                      : "huaxia:newtab"
                  }
                  onHome={() => setTabUrl(tab.id, null)}
                  onClosed={() => closeTab(tab.id)}
                />
              ) : (
                <NewTab
                  libraryName={library.libraryName ?? undefined}
                  onNavigate={navigate}
                  onOpenTab={(kind) => openTab(kind as TabKind)}
                  onBrowse={(url) => setTabUrl(tab.id, url)}
                />
              )}
            </div>
          ))}
        {/* 画廊标签：每本一个独立 Reader 实例，不共用主页的 readerCard。 */}
        {tabs
          .filter((tab) => tab.kind === "gallery")
          .map((tab) => (
            <div
              key={tab.id}
              style={{
                position: "absolute",
                inset: 0,
                visibility: activeId === tab.id ? "visible" : "hidden",
                contentVisibility: activeId === tab.id ? "visible" : "hidden",
              }}
            >
              <GalleryTab archiveId={tab.archiveId ?? 0} onClose={() => closeTab(tab.id)} />
            </div>
          ))}
      </div>
      {/* 标签右键菜单：切换分组 / 拆出窗口（M5）。 */}
      {tabMenu && (() => {
        const tab = tabs.find((entry) => entry.id === tabMenu.id);
        if (!tab) return null;
        const currentGroup = groupIdOf(tab);
        const itemStyle: React.CSSProperties = {
          display: "block", width: "100%", textAlign: "left", border: "none",
          background: "transparent", color: "inherit", cursor: "pointer",
          padding: "7px 10px", fontSize: 12.5,
        };
        return (
          <div className="tab-context-backdrop" onClick={() => setTabMenu(null)}>
            <div
              className="tab-context-menu"
              style={{
                position: "fixed",
                left: Math.min(tabMenu.x, window.innerWidth - 210),
                top: Math.min(tabMenu.y, window.innerHeight - 240),
                zIndex: 60,
                minWidth: 186,
                padding: 5,
                borderRadius: 10,
                border: "1px solid var(--border, #2c2c34)",
                background: "var(--bg-elev, #1c1c22)",
                boxShadow: "0 16px 40px -18px rgba(0,0,0,0.9)",
              }}
              onClick={(event) => event.stopPropagation()}
            >
              <div style={{ padding: "4px 10px 6px", fontSize: 11, color: "var(--text-dim, #9a9aa4)" }}>
                移到分组
              </div>
              {groups.map((group) => (
                <button
                  key={group.id}
                  type="button"
                  style={{
                    ...itemStyle,
                    fontWeight: group.id === currentGroup ? 700 : 400,
                    color: group.id === currentGroup ? group.color : "inherit",
                  }}
                  onClick={() => {
                    setGroupOf((prev) => ({ ...prev, [tab.id]: group.id }));
                    setGroups((prev) => prev.map((g) => g.id === group.id ? { ...g, collapsed: false } : g));
                    setTabMenu(null);
                  }}
                >
                  <span style={{ display: "inline-block", width: 8, height: 8, borderRadius: "50%", background: group.color, marginRight: 8 }} />
                  {group.title}
                </button>
              ))}
              <div style={{ height: 1, background: "var(--border, #2c2c34)", margin: "5px 0" }} />
              <button
                type="button"
                style={itemStyle}
                onClick={() => {
                  const group = groups.find((g) => g.id === currentGroup);
                  if (group) toggleGroup(group.id);
                  setTabMenu(null);
                }}
              >
                折叠/展开当前组
              </button>
              {tab.kind !== "home" && (
                <button
                  type="button"
                  style={itemStyle}
                  title="把标签拖出标签栏也可以拆窗"
                  onClick={() => {
                    detachTab(tab.id, 0, 0);
                    setTabMenu(null);
                  }}
                >
                  ⤢ 拆出为独立窗口
                </button>
              )}
            </div>
          </div>
        );
      })()}
      {/* 启动遮罩：盖住内容，点一下才显示（--masked 的本地雏形）。 */}
      {masked && (
        <div
          className="startup-mask"
          role="presentation"
          onClick={() => setMasked(false)}
          title="点击显示画匣"
        >
          <div className="startup-mask-card">
            <span className="startup-mask-dot" aria-hidden="true" />
            <strong>画匣已盖住</strong>
            <p>点击任意处继续</p>
          </div>
        </div>
      )}
    </div>
  );
}
