import { useState } from "react";
import { api } from "../api";
import type { LibraryState, ViewMode } from "../types";
import { formatBytes } from "../format";
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
const KEY_ROWS: { keys: string; action: string; group: string }[] = [
  { keys: "← → ↑ ↓ / 空格 / PgUp PgDn", action: "翻页（空格 = 暂停/继续播放）", group: "阅读器" },
  { keys: "Home / End", action: "第一页 / 最后一页", group: "阅读器" },
  { keys: "+ / - / 0 / Ctrl+滚轮", action: "放大 / 缩小 / 适应页面", group: "阅读器" },
  { keys: "D", action: "双页开合", group: "阅读器" },
  { keys: "F", action: "沉浸模式（连标题栏一起收掉）", group: "阅读器" },
  { keys: "P", action: "GIF 播放（动图播完整段再翻页）", group: "阅读器" },
  { keys: "Ctrl+1~9 / Ctrl+0", action: "幻灯播放每 N 秒一页 / 关闭幻灯", group: "阅读器" },
  { keys: "B / L / I", action: "加书签 / 书签列表 / 详情浮层", group: "阅读器" },
  { keys: "[ / ]", action: "上一本 / 下一本", group: "阅读器" },
  { keys: "Ctrl+C", action: "复制当前页到剪贴板", group: "阅读器" },
  { keys: "Ctrl+Enter", action: "在资源管理器中定位画集", group: "阅读器" },
  { keys: "右键", action: "阅读器菜单（翻页/缩放/播放/图片操作）", group: "阅读器" },
  { keys: "Esc", action: "退出阅读 / 关面板", group: "阅读器" },
  { keys: "Ctrl+H", action: "老板键：立刻藏起画匣窗口", group: "全局" },
  { keys: "Ctrl+Shift+H", action: "老板键呼回（全局热键，藏起后也能按）", group: "全局" },
  { keys: "主目录右键", action: "导入 / 刷新 / 视图 / 设置", group: "画廊" },
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
                <h3>快捷键</h3>
                <table className="settings-keys">
                  <tbody>
                    {KEY_ROWS.map((row) => (
                      <tr key={`${row.group}-${row.keys}`}>
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
