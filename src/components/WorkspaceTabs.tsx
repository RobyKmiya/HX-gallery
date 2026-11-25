/**
 * 工作台多标签（P1，方案见 docs/浏览器内置阅读器方案.md）。
 *
 * 形态：顶部标签条，主页标签 = 库主目录界面（LibraryView 原样保留），
 * 其余标签 = 内建浏览器（原生子 webview 挂 main 窗，React 只量占位矩形）。
 *
 * 关键规则：
 * - 原生子 webview 是独立 HWND，React 模态盖不住 → 任何模态打开时必须
 *   notifyModalOpen() 让浏览器 host 整体隐藏，关掉再恢复；
 * - 引擎切标签只隐藏不销毁，回来原样（登录态/滚动都在）；
 * - 关浏览器标签 = d2_embed_close，绝不碰宿主窗。
 */
import { useEffect, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { api } from "../api";
import type { LibraryState } from "../types";
import LibraryView from "./LibraryView";

export interface WorkTab {
  id: string;
  kind: TabKind;
  title: string;
}

export type TabKind =
  | "home"
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
  browser: "🌐",
  tags: "#️⃣",
  trash: "🗑",
  release: "🚀",
  settings: "⚙️",
  net: "⬇️",
};

const TAB_TITLE: Record<TabKind, string> = {
  home: "主页",
  browser: "浏览器",
  tags: "标签中心",
  trash: "回收站",
  release: "发版",
  settings: "设置",
  net: "网络画集",
};

import { EVENT_MODAL } from "../modalBus";

function browserTabTitle(index: number): string {
  return index === 0 ? "浏览器" : `浏览器 ${index + 1}`;
}

function BrowserHost({ visible }: { visible: boolean }) {
  const holderRef = useRef<HTMLDivElement>(null);
  const [label, setLabel] = useState<string | null>(null);
  const modalCount = useRef(0);
  const [modalTick, setModalTick] = useState(0);

  // 引擎生命周期：挂载时开，卸载时拆（切标签不卸载，只切 visible）。
  useEffect(() => {
    let dead = false;
    let mine: string | null = null;
    void (async () => {
      try {
        const opened = await api.d2EmbedOpen({ url: "https://e-hentai.org/", adblock: true });
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
  }, []);

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
  const [tabs, setTabs] = useState<WorkTab[]>([{ id: "home", kind: "home", title: "主页" }]);
  const [activeId, setActiveId] = useState("home");
  const browserCount = tabs.filter((tab) => tab.kind === "browser").length;

  /** 统一开标签入口：单例已存在则召回；可多开的每次新建。 */
  const openTab = (kind: TabKind, title?: string, forceNew = false) => {
    if (!forceNew && SINGLETON.includes(kind)) {
      const existing = tabs.find((tab) => tab.kind === kind);
      if (existing) {
        setActiveId(existing.id);
        return existing.id;
      }
    }
    const id = `${kind}-${Date.now().toString(36)}`;
    const count = tabs.filter((tab) => tab.kind === kind).length;
    const name = title ?? (count > 0 ? `${TAB_TITLE[kind]} ${count + 1}` : TAB_TITLE[kind]);
    setTabs((prev) => [...prev, { id, kind, title: name }]);
    setActiveId(id);
    return id;
  };

  const addBrowserTab = () => {
    const id = `browser-${Date.now().toString(36)}`;
    const title = browserTabTitle(browserCount);
    setTabs((prev) => [...prev, { id, kind: "browser", title }]);
    setActiveId(id);
  };

  const closeTab = (id: string) => {
    if (id === "home") return;
    setTabs((prev) => {
      const next = prev.filter((tab) => tab.id !== id);
      if (activeId === id) setActiveId(next[next.length - 1]?.id ?? "home");
      return next;
    });
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
          return (
            <div
              key={tab.id}
              onClick={() => setActiveId(tab.id)}
              style={{
                display: "flex",
                alignItems: "center",
                gap: 6,
                padding: "5px 10px",
                borderRadius: "8px 8px 0 0",
                cursor: "pointer",
                fontSize: 12.5,
                whiteSpace: "nowrap",
                maxWidth: 180,
                minWidth: 74,
                overflow: "hidden",
                background: active ? "var(--bg, #14141a)" : "transparent",
                color: active ? "var(--text, #e8e8ee)" : "var(--text-dim, #9a9aa4)",
                borderTop: active ? "2px solid var(--accent, #7c93ff)" : "2px solid transparent",
              }}
              title={tab.kind === "home" ? "库主目录（不可关闭）" : "内建浏览器标签"}
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
        {tabs.map((tab) => {
          const active = tab.id === activeId;
          return (
            <div
              key={tab.id}
              style={{
                position: "absolute",
                inset: 0,
                visibility: active ? "visible" : "hidden",
              }}
            >
              {tab.kind === "home" ? (
                <LibraryView
                  library={library}
                  onLibraryChanged={onLibraryChanged}
                  onSwitchLibrary={onSwitchLibrary}
                  onOpenTab={(kind, title) => openTab(kind as TabKind, title)}
                />
              ) : (
                <BrowserHost visible={active} />
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
