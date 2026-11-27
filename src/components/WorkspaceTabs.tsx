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
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { api } from "../api";
import type { LibraryState, NavIntent } from "../types";
import LibraryView from "./LibraryView";
import NewTab from "./NewTab";
import GalleryTab from "./GalleryTab";
import {
  TAB_SETTINGS_EVENT,
  readPersistedTabs,
  readStartupTarget,
  readTabSettings,
  savePersistedTabs,
  type StartupTarget,
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

/** 启动时决定初始标签集合：主页 / 上次的非浏览器标签 / 指定收藏（仍是主页）。 */
function initialTabs(): WorkTab[] {
  const settings = readTabSettings();
  if (settings.startupMode === "last") {
    const persisted = readPersistedTabs();
    if (persisted) {
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
  const settings = readTabSettings();
  if (settings.startupMode === "last") {
    const persisted = readPersistedTabs();
    if (persisted) {
      const match = tabs.find((tab) => tab.kind === persisted.active);
      if (match) return match.id;
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
        const opened = await api.d2EmbedOpen({
          url,
          adblock: true,
          home,
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

  // 标签集合落盘（跳过首帧，避免用默认主页覆盖「上次的标签」）。
  useEffect(() => {
    if (!hydratedRef.current) {
      hydratedRef.current = true;
      return;
    }
    const kind = tabs.find((tab) => tab.id === activeId)?.kind ?? "home";
    savePersistedTabs({
      kinds: tabs
        .filter((tab) => tab.kind !== "browser" && tab.kind !== "gallery")
        .map((tab) => tab.kind),
      active: kind,
    });
  }, [tabs, activeId]);

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

  // 详情页点「源链接」→ 这里开网页标签（CustomEvent，避免一层层传回调）。
  useEffect(() => {
    const handler = (event: Event) => {
      const url = (event as CustomEvent<string>).detail;
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

  /** 落点排序：把 from 标签移动到 to 标签的位置（不是硬交换，同标签中心）。 */
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
  };

  /** 标签条拖拽排序（指针事件；HTML5 draggable 在 WebView2 里会被系统拖放吞掉）。 */
  const tabDrag = usePointerDrag({
    axis: "x",
    onDrop: reorderTabs,
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
        {tabs.map((tab) => {
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
                // 视觉统一：定宽 + 省略，主页与浏览器标签不再长短不一。
                width: 148,
                flex: "0 0 148px",
                overflow: "hidden",
                background: active ? "var(--bg, #14141a)" : "transparent",
                color: active ? "var(--text, #e8e8ee)" : "var(--text-dim, #9a9aa4)",
                borderTop: active ? "2px solid var(--accent, #7c93ff)" : "2px solid transparent",
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
