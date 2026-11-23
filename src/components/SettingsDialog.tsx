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
import {
  DEFAULT_GESTURES,
  GESTURE_LABELS,
  loadGestures,
  saveGestures,
  type GestureAction,
  type GestureSlot,
} from "../gestures";
import { AlertIcon, BookIcon, CloseIcon, FolderIcon, RefreshIcon, TagIcon } from "./Icons";

type SettingsTab = "general" | "keys" | "gestures" | "storage" | "guide" | "more";

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
  { key: "gestures", label: "鼠标手势" },
  { key: "storage", label: "存储" },
  { key: "guide", label: "新手教程" },
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
  const [gestures, setGestures] = useState(() => loadGestures());
  /** 双击画集卡片的行为：immersive / normal / off（11-23 需求）。 */
  const [cardDoubleOpen, setCardDoubleOpen] = useState(() => {
    try {
      return window.localStorage.getItem("huaxia.ui.cardDoubleOpen") ?? "immersive";
    } catch {
      return "immersive";
    }
  });
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
                  <span>双击画集卡片</span>
                  <select
                    value={cardDoubleOpen}
                    onChange={(event) => {
                      const next = event.target.value;
                      setCardDoubleOpen(next);
                      try {
                        window.localStorage.setItem("huaxia.ui.cardDoubleOpen", next);
                      } catch {
                        // 存不了就只在本次生效
                      }
                    }}
                  >
                    <option value="immersive">直接阅读（沉浸）</option>
                    <option value="normal">直接阅读（普通）</option>
                    <option value="off">不处理（只弹详情）</option>
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

            {tab === "gestures" && (
              <>
                <h3>鼠标手势（阅读器内生效）</h3>
                <div className="keys-editor">
                  {(Object.keys(GESTURE_LABELS) as GestureSlot[]).map((slot) => (
                    <div className="keys-row" key={slot}>
                      <span className="keys-row-label">{GESTURE_LABELS[slot]}</span>
                      <select
                        className="keys-select"
                        value={gestures[slot]}
                        onChange={(event) => {
                          const next = {
                            ...gestures,
                            [slot]: event.target.value as GestureAction,
                          };
                          setGestures(next);
                          saveGestures(next);
                        }}
                      >
                        <option value="none">不做任何事</option>
                        {(Object.keys(SHORTCUT_LABELS) as ShortcutAction[]).map(
                          (action) => (
                            <option key={action} value={action}>
                              {SHORTCUT_LABELS[action]}
                            </option>
                          ),
                        )}
                      </select>
                    </div>
                  ))}
                </div>
                <button
                  type="button"
                  className="btn btn-quiet btn-compact"
                  onClick={() => {
                    const next = { ...DEFAULT_GESTURES };
                    setGestures(next);
                    saveGestures(next);
                  }}
                >
                  恢复默认手势
                </button>
                <p className="settings-hint">
                  点击分区（左/中/右）在阅读器里直接拖动两条竖线调整宽度；缩放只走
                  Ctrl+滚轮，普通滚轮不再改缩放。
                </p>
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

            {tab === "guide" && (
              <>
                <h3>三步上手</h3>
                <ol className="guide-steps">
                  <li>
                    <strong>选一个库目录</strong>——建议放在容量够的盘，比如{" "}
                    <code>D://画匣库</code>。画集压缩包会<strong>复制</strong>进这里（原文件不动），
                    索引、封面、标签都存在库里的 <code>.gallery</code>。
                  </li>
                  <li>
                    <strong>导入</strong>——点「导入压缩包」选 ZIP/CBZ/7Z/RAR，
                    或直接把文件拖进窗口；一个压缩包 = 一个画集，同包多图 = 多页。
                  </li>
                  <li>
                    <strong>双击开始看</strong>——阅读里 <code>←</code>/<code>→</code>{" "}
                    翻页，<code>F</code> 沉浸，<code>右键</code> 出菜单（缩放/播放/复制/壁纸）。
                  </li>
                </ol>

                <h3>阅读器速查</h3>
                <ul className="guide-list">
                  <li>翻页：方向键 / 空格 / PgUp、PgDn；首末页 Home、End</li>
                  <li>缩放：<code>+</code> <code>-</code> <code>0</code>，鼠标侧只认 <code>Ctrl+滚轮</code></li>
                  <li><code>D</code> 双页 ｜ <code>F</code> 沉浸（双击画面也行）｜ <code>T</code> 缩略图栏</li>
                  <li><code>Shift+P</code> GIF 播放（动图播完再翻页）｜ <code>Ctrl+1~9</code> 幻灯每 N 秒一页</li>
                  <li><code>B</code> 书签 ｜ <code>L</code> 书签列表 ｜ <code>Alt+←/→</code> 上下本</li>
                  <li><strong>收藏页</strong>：<code>P</code> 单页收藏；双页时 <code>[</code> 收藏左页、<code>]</code> 收藏右页</li>
                  <li>点击画面：左区上一页、中区开目录、右区下一页（边界可拖动，见「鼠标手势」）</li>
                  <li><code>Ctrl+C</code> 复制当前页 ｜ <code>Ctrl+Enter</code> 资源管理器定位</li>
                  <li><code>Ctrl+H</code> 老板键藏窗 ｜ <code>Ctrl+Shift+H</code> 呼回</li>
                  <li>鼠标后退侧键：沉浸态先退沉浸，再按退出阅读</li>
                </ul>

                <h3>几个常用开关</h3>
                <ul className="guide-list">
                  <li>双击画集卡片直接开看（沉浸/普通可选）：设置 → 双击画集卡片</li>
                  <li>网页抓取登录失败：D2 面板里用「Cookie 登录」粘贴浏览器 cookie</li>
                </ul>

                <h3>鼠标右键</h3>
                <ul className="guide-list">
                  <li>阅读器里右键：翻页 / 缩放 / 播放 / 复制页 / 设为壁纸</li>
                  <li>画集卡片右键：加入待看目录 / 移入回收站 / 打开所在位置</li>
                  <li>画廊空白处右键：导入 / 刷新 / 切视图 / 打开各面板 / 设置</li>
                </ul>

                <h3>常见问题</h3>
                <ul className="guide-list">
                  <li><strong>会不会上传我的图？</strong> 完全离线，不联网，不上传任何文件。</li>
                  <li><strong>库里存的是什么？</strong> 原始压缩包 + <code>.gallery</code>{" "}
                    里的索引与封面缓存；删库目录 = 全没了，挪目录请整目录搬。</li>
                  <li><strong>快捷键能改吗？</strong> 能，「快捷键」页点键位后按新组合即可。</li>
                  <li><strong>合集怎么建？</strong> 选中几个画集 → 右键「加入合集」/ 新建合集，
                    详情里能调整集数顺序、改每话名字、设封面。</li>
                  <li><strong>标签太多怎么办？</strong> 标签中心可批量选中合并、一键保存全部修改。</li>
                </ul>
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
