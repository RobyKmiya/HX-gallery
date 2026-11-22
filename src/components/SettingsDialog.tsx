import { useEffect, useState } from "react";
import { api } from "../api";
import type { LibraryState, ViewMode } from "../types";
import { formatBytes } from "../format";
import {
  DEFAULT_SHORTCUTS,
  SHORTCUT_LABELS,
  describeBinding,
  eventToBinding,
  loadShortcuts,
  saveShortcuts,
  type ShortcutAction,
} from "../shortcuts";
import { AlertIcon, BookIcon, CloseIcon, FolderIcon, RefreshIcon, TagIcon } from "./Icons";

type SettingsTab = "general" | "keys" | "storage" | "more";

interface SettingsDialogProps {
  library: LibraryState;
  viewMode: ViewMode;
  onViewModeChange: (mode: ViewMode) => void;
  detailDock: "float" | "dock";
  onDetailDockChange: (dock: "float" | "dock") => void;
  onOpenTagCenter: () => void;
  onOpenD2: () => void;
  onOpenRelease: () => void;
  onClose: () => void;
}

const TABS: { key: SettingsTab; label: string }[] = [
  { key: "general", label: "通用" },
  { key: "keys", label: "快捷键" },
  { key: "storage", label: "存储" },
  { key: "more", label: "更多入口" },
];

/** 阅读器/全局键位表（只读展示；改键位等有真实需求再做）。 */
/** 结构键：与布局/播放语义绑定，不参与自定义。 */
const FIXED_KEYS: { keys: string; action: string }[] = [
  { keys: "空格", action: "下一页 / 播放中暂停（按状态自动切换）" },
  { keys: "Ctrl+1~9 · Ctrl+0", action: "幻灯播放每 N 秒一页 / 关闭幻灯" },
  { keys: "Ctrl+H · Ctrl+Shift+H", action: "老板键藏窗 / 全局呼回" },
  { keys: "[ · ]", action: "上一本 / 下一本画集" },
  { keys: "Esc", action: "关菜单 / 退出阅读" },
  { keys: "右键", action: "阅读器菜单" },
];

/**
 * 统一设置对话框：通用 / 快捷键 / 存储 / 更多入口。
 * 标签中心、网页抓取、更新本来就是独立面板，这里只做入口不搬功能。
 */
export default function SettingsDialog({
  library,
  viewMode,
  onViewModeChange,
  detailDock,
  onDetailDockChange,
  onOpenTagCenter,
  onOpenD2,
  onOpenRelease,
  onClose,
}: SettingsDialogProps) {
  const [tab, setTab] = useState<SettingsTab>("general");
  const [bindings, setBindings] = useState(() => loadShortcuts());
  /** 正在录制的动作；null = 不在录制。 */
  const [recording, setRecording] = useState<ShortcutAction | null>(null);
  const [keyHint, setKeyHint] = useState("");

  // 录制：按下的第一枚非修饰键直接作为绑定；Esc 取消。
  useEffect(() => {
    if (!recording) return;
    const onKeyDown = (event: KeyboardEvent) => {
      event.preventDefault();
      event.stopPropagation();
      if (event.key === "Escape") {
        setRecording(null);
        setKeyHint("");
        return;
      }
      const binding = eventToBinding(event);
      if (!binding) return;
      const conflict = (
        Object.keys(bindings) as ShortcutAction[]
      ).find((action) => action !== recording && bindings[action] === binding);
      if (conflict) {
        setKeyHint(`「${binding}」已绑定给「${SHORTCUT_LABELS[conflict]}」，换一个`);
        return;
      }
      const next = { ...bindings, [recording]: binding };
      setBindings(next);
      saveShortcuts(next);
      setRecording(null);
      setKeyHint("");
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [recording, bindings]);

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <section
        className="modal settings-dialog"
        role="dialog"
        aria-modal="true"
        aria-label="画匣设置"
        onClick={(event) => event.stopPropagation()}
      >
        <div className="modal-title-row">
          <div>
            <p className="eyebrow">SETTINGS</p>
            <h2>画匣设置</h2>
          </div>
          <button
            type="button"
            className="icon-button"
            onClick={onClose}
            aria-label="关闭设置"
          >
            <CloseIcon />
          </button>
        </div>

        <div className="settings-body">
          <nav className="settings-nav" aria-label="设置分组">
            {TABS.map((entry) => (
              <button
                key={entry.key}
                type="button"
                className={`settings-nav-item${tab === entry.key ? " is-active" : ""}`}
                onClick={() => setTab(entry.key)}
              >
                {entry.label}
              </button>
            ))}
          </nav>

          <div className="settings-content">
            {tab === "general" && (
              <>
                <h3>视图</h3>
                <label className="settings-row">
                  <span>默认浏览方式</span>
                  <select
                    value={viewMode}
                    onChange={(event) =>
                      onViewModeChange(event.target.value === "pages" ? "pages" : "scroll")
                    }
                  >
                    <option value="scroll">网格滚动（一次看全部）</option>
                    <option value="pages">分页浏览（翻页看）</option>
                  </select>
                </label>
                <label className="settings-row">
                  <span>详情浮层停靠</span>
                  <select
                    value={detailDock}
                    onChange={(event) =>
                      onDetailDockChange(event.target.value === "dock" ? "dock" : "float")
                    }
                  >
                    <option value="float">浮动（盖在画廊上）</option>
                    <option value="dock">停靠（挤开画廊）</option>
                  </select>
                </label>
              </>
            )}

            {tab === "keys" && (
              <>
                <h3>阅读器快捷键（点击右侧按钮后按新键）</h3>
                <div className="keys-editor">
                  {(Object.keys(SHORTCUT_LABELS) as ShortcutAction[]).map(
                    (action) => (
                      <div className="keys-row" key={action}>
                        <span className="keys-row-label">
                          {SHORTCUT_LABELS[action]}
                        </span>
                        <button
                          type="button"
                          className={`keys-binding${
                            recording === action ? " is-recording" : ""
                          }`}
                          onClick={() =>
                            setRecording(recording === action ? null : action)
                          }
                          title="点击后按新的按键组合（Esc 取消）"
                        >
                          {recording === action
                            ? "按新键…"
                            : describeBinding(bindings[action])}
                        </button>
                        <button
                          type="button"
                          className="nav-mini"
                          title="清除绑定"
                          onClick={() => {
                            const next = { ...bindings, [action]: "" };
                            setBindings(next);
                            saveShortcuts(next);
                          }}
                        >
                          ×
                        </button>
                      </div>
                    ),
                  )}
                </div>
                {keyHint && <p className="keys-hint">{keyHint}</p>}
                <button
                  type="button"
                  className="btn btn-quiet btn-compact"
                  onClick={() => {
                    const next = { ...DEFAULT_SHORTCUTS };
                    setBindings(next);
                    saveShortcuts(next);
                    setKeyHint("已恢复默认键位");
                  }}
                >
                  恢复默认
                </button>

                <h3>结构键（固定）</h3>
                <table className="settings-keys">
                  <tbody>
                    {FIXED_KEYS.map((row) => (
                      <tr key={row.keys}>
                        <td className="settings-keys-action">{row.action}</td>
                        <td className="settings-keys-keys">{row.keys}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </>
            )}

            {tab === "storage" && (
              <>
                <h3>当前库</h3>
                <p className="settings-kv">
                  <span>库名</span>
                  <strong>{library.libraryName ?? "未命名库"}</strong>
                </p>
                <p className="settings-kv">
                  <span>路径</span>
                  <strong title={library.libraryPath ?? ""}>{library.libraryPath}</strong>
                </p>
                <p className="settings-kv">
                  <span>画集</span>
                  <strong>{library.archiveCount} 个 · {formatBytes(library.totalSize)}</strong>
                </p>
                {library.missingCount > 0 && (
                  <p className="settings-kv is-warn">
                    <AlertIcon width={12} height={12} />
                    <span>{library.missingCount} 个文件缺失</span>
                  </p>
                )}
                <button
                  type="button"
                  className="btn btn-quiet btn-compact"
                  disabled={!library.libraryPath}
                  onClick={() => {
                    if (library.libraryPath) {
                      void api.revealInExplorer(library.libraryPath);
                    }
                  }}
                >
                  <FolderIcon width={13} height={13} />
                  在资源管理器中显示
                </button>
                <p className="settings-hint">
                  更换库目录请用左上角的库选择器；封面缓存与备份都在库目录的 .gallery 下。
                </p>
              </>
            )}

            {tab === "more" && (
              <>
                <h3>更多面板</h3>
                <button type="button" className="btn btn-quiet" onClick={onOpenTagCenter}>
                  <TagIcon width={13} height={13} />
                  标签中心（分类 / 翻译 / 标签导图）
                </button>
                <button type="button" className="btn btn-quiet" onClick={onOpenD2}>
                  <BookIcon width={13} height={13} />
                  网页抓取（D2 下载器）
                </button>
                <button type="button" className="btn btn-quiet" onClick={onOpenRelease}>
                  <RefreshIcon width={13} height={13} />
                  检查更新（更新画匣）
                </button>
                <p className="settings-hint">
                  散图收藏夹、待看目录、回收站都在左侧导航栏。
                </p>
              </>
            )}
          </div>
        </div>
      </section>
    </div>
  );
}
