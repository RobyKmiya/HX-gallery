import { useEffect, useState } from "react";
import { api } from "../api";
import { notifyModalOpen, notifyModalClose } from "../modalBus";
import {
  readTabSettings,
  saveTabSettings,
  type TabSettings,
  type StartupMode,
  type StartupTargetKind,
} from "../tabSettings";
import type { Collection, LibraryState, ViewMode } from "../types";
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
import WallpaperLockSettings from "./WallpaperLockSettings";
import BrowserDataSettings from "./BrowserDataSettings";
import UserscriptSettings from "./UserscriptSettings";
import AdblockSettings from "./AdblockSettings";
import BrowserPrefsSettings from "./BrowserPrefsSettings";

type SettingsTab =
  | "general"
  | "reading"
  | "wallpaper"
  | "web"
  | "browser"
  | "userscript"
  | "adblock"
  | "keys"
  | "gestures"
  | "storage"
  | "backup"
  | "diag"
  | "guide"
  | "more";

interface LibraryOps {
  importing: boolean;
  scanning: boolean;
  onPickFiles: () => void;
  onPickFolder: () => void;
  onRunScan: () => void;
  onSwitchLibrary: () => void;
}

interface SettingsDialogProps {
  library: LibraryState;
  /** 库操作（导入/重扫/切换目录）：从侧栏迁移过来，侧栏只留浏览。 */
  libraryOps?: LibraryOps;
  /** 收藏夹列表：启动页「指定收藏」下拉用。 */
  collections?: Collection[];
  viewMode: ViewMode;
  onViewModeChange: (mode: ViewMode) => void;
  detailDock: "float" | "dock";
  onDetailDockChange: (dock: "float" | "dock") => void;
  onOpenTagCenter: () => void;
  onOpenD2: () => void;
  onOpenRelease: () => void;
  /** 嵌入标签：不画全屏遮罩、不显示自带关闭按钮。 */
  embedded?: boolean;
  /** 始终挂载的面板：非激活时不监听快捷键录制。 */
  active?: boolean;
  onClose: () => void;
}

const TABS: { key: SettingsTab; label: string }[] = [
  { key: "general", label: "通用" },
  { key: "reading", label: "阅读" },
  { key: "wallpaper", label: "壁纸锁" },
  { key: "web", label: "匣浏览器" },
  { key: "browser", label: "浏览器数据" },
  { key: "userscript", label: "用户脚本" },
  { key: "adblock", label: "广告拦截" },
  { key: "keys", label: "快捷键" },
  { key: "gestures", label: "鼠标手势" },
  { key: "storage", label: "存储" },
  { key: "backup", label: "备份迁移" },
  { key: "diag", label: "诊断" },
  { key: "guide", label: "新手教程" },
  { key: "more", label: "更多入口" },
];

/** 阅读器/全局键位表（只读展示；改键位等有真实需求再做）。 */
/** 结构键：与布局/播放语义绑定，不参与自定义。 */
const FIXED_KEYS: { keys: string; action: string }[] = [
  { keys: "空格", action: "下一页 / 播放中暂停（按状态自动切换）" },
  { keys: "Ctrl+1~9 · Ctrl+0", action: "幻灯播放每 N 秒一页 / 关闭幻灯" },
  // 老板键两项已进上面的自定义表（11-24：管理员要求能改绑定）。
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
  libraryOps,
  collections = [],
  viewMode,
  onViewModeChange,
  detailDock,
  onDetailDockChange,
  onOpenTagCenter,
  onOpenD2,
  onOpenRelease,
  embedded = false,
  active = true,
  onClose,
}: SettingsDialogProps) {
  const [tab, setTab] = useState<SettingsTab>("general");
  const [bindings, setBindings] = useState(() => loadShortcuts());
  // 模态让位（P1 工作台）：原生子 webview 盖不住 React 模态，
  // 打开/关闭时广播，让内建浏览器 host 整体隐藏/恢复。
  useEffect(() => {
    if (embedded) return;
    notifyModalOpen();
    return () => notifyModalClose();
  }, [embedded]);
  // 呼回键的真值在 Rust（注册过的才算数）：打开面板时以 Rust 为准校正一遍，
  // 避免另一处改过之后 localStorage 与实际注册的键不一致。
  useEffect(() => {
    void api
      .getBossRecall()
      .then((binding) => {
        if (!binding) return;
        setBindings((prev) =>
          prev.bossRecall === binding ? prev : { ...prev, bossRecall: binding },
        );
        saveShortcuts({ ...loadShortcuts(), bossRecall: binding });
      })
      .catch(() => {});
  }, []);
  const [gestures, setGestures] = useState(() => loadGestures());
  /** 双击画集卡片的行为：immersive / normal / off（11-23 需求）。 */
  const [cardDoubleOpen, setCardDoubleOpen] = useState(() => {
    try {
      return window.localStorage.getItem("huaxia.ui.cardDoubleOpen") ?? "immersive";
    } catch {
      return "immersive";
    }
  });
  /** 标签范式三项设置（新开行为 / 启动页 / 启动遮罩），与标签壳共用一份。 */
  const [tabSettings, setTabSettings] = useState<TabSettings>(() => readTabSettings());
  const updateTabSettings = (patch: Partial<TabSettings>) => {
    const next = { ...tabSettings, ...patch };
    setTabSettings(next);
    saveTabSettings(next);
  };
  /** 正在录制的动作；null = 不在录制。 */
  const [recording, setRecording] = useState<ShortcutAction | null>(null);
  const [keyHint, setKeyHint] = useState("");

  // 诊断面板（1.2.4 提案 #1）：一键自检 + 诊断包导出。
  const [healthItems, setHealthItems] = useState<
    { key: string; ok: boolean; detail: string }[] | null
  >(null);
  const [healthBusy, setHealthBusy] = useState(false);
  const [diagBundlePath, setDiagBundlePath] = useState<string | null>(null);
  const [diagBusy, setDiagBusy] = useState(false);
  const [diagError, setDiagError] = useState<string | null>(null);

  const runHealthCheck = async () => {
    setHealthBusy(true);
    setDiagError(null);
    try {
      setHealthItems(await api.healthCheck());
    } catch (error) {
      setDiagError(`自检失败：${String(error)}`);
    } finally {
      setHealthBusy(false);
    }
  };

  const exportDiag = async () => {
    setDiagBusy(true);
    setDiagError(null);
    try {
      setDiagBundlePath(await api.exportDiagBundle());
    } catch (error) {
      setDiagError(`导出失败：${String(error)}`);
    } finally {
      setDiagBusy(false);
    }
  };

  // ── 备份与迁移（1.2.5 提案 M2）──
  interface BackupEntry {
    name: string;
    path: string;
    size: number;
    kind: string;
    created_ms: number;
  }
  interface RestoreReport {
    restored_db: boolean;
    restored_settings: boolean;
    archive_count: number;
    warnings: string[];
  }
  const [backupList, setBackupList] = useState<BackupEntry[] | null>(null);
  const [backupBusy, setBackupBusy] = useState(false);
  const [backupReport, setBackupReport] = useState<string | null>(null);
  const [restoreConfirm, setRestoreConfirm] = useState<string | null>(null);
  const [restoreReport, setRestoreReport] = useState<RestoreReport | null>(null);
  const [restoreBusy, setRestoreBusy] = useState(false);
  const [backupError, setBackupError] = useState<string | null>(null);

  const KIND_LABEL: Record<string, string> = {
    export: "手动导出",
    "daily-db": "每日库快照",
    "daily-settings": "每日设置快照",
  };

  const refreshBackups = async () => {
    try {
      setBackupList(await api.listBackups());
    } catch (error) {
      setBackupError(`读取备份列表失败：${String(error)}`);
    }
  };

  useEffect(() => {
    if (tab === "backup") void refreshBackups();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tab]);

  const runBackupExport = async () => {
    setBackupBusy(true);
    setBackupError(null);
    setBackupReport(null);
    try {
      const report = await api.exportBackup();
      setBackupReport(
        `备份完成：${report.path}（${report.archive_count} 本画集的库快照 + 全部设置）`,
      );
      await refreshBackups();
    } catch (error) {
      setBackupError(`备份失败：${String(error)}`);
    } finally {
      setBackupBusy(false);
    }
  };

  const runRestore = async (path: string) => {
    setRestoreBusy(true);
    setBackupError(null);
    setRestoreConfirm(null);
    try {
      const report = await api.restoreBackup(path);
      setRestoreReport(report);
      await refreshBackups();
    } catch (error) {
      setBackupError(`恢复失败：${String(error)}`);
    } finally {
      setRestoreBusy(false);
    }
  };

  // 录制：按下的第一枚非修饰键直接作为绑定；Esc 取消。
  useEffect(() => {
    if (!active || !recording) return;
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
      // 呼回键是系统级热键，要推给 Rust 注册；注册失败就回滚并提示。
      if (recording === "bossRecall") {
        void api.setBossRecall(binding).catch((error) => {
          setKeyHint(`呼回键「${binding}」注册失败：${String(error)}`);
          setBindings((prev) => ({ ...prev, [recording]: bindings[recording] }));
          saveShortcuts({ ...next, [recording]: bindings[recording] });
        });
      }
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [active, recording, bindings]);

  return (
    <div
      className={embedded ? "panel-embedded" : "modal-backdrop"}
      onClick={embedded ? undefined : onClose}
    >
      <section
        className={`modal settings-dialog${embedded ? " is-embedded" : ""}`}
        role={embedded ? "region" : "dialog"}
        aria-modal={embedded ? undefined : true}
        aria-label="画匣设置"
        onClick={(event) => event.stopPropagation()}
      >
        <div className="modal-title-row">
          <div>
            <p className="eyebrow">SETTINGS</p>
            <h2>画匣设置</h2>
          </div>
          {!embedded && (
            <button
              type="button"
              className="icon-button"
              onClick={onClose}
              aria-label="关闭设置"
            >
              <CloseIcon />
            </button>
          )}
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

                <h3>标签</h3>
                <label className="settings-row">
                  <span>打开新页面时</span>
                  <select
                    value={tabSettings.newBehavior}
                    onChange={(event) =>
                      updateTabSettings({
                        newBehavior: event.target.value === "reuse" ? "reuse" : "new",
                      })
                    }
                  >
                    <option value="new">新建标签</option>
                    <option value="reuse">复用已有标签</option>
                  </select>
                </label>
                <label className="settings-row">
                  <span>新标签打开</span>
                  <select
                    value={tabSettings.newTabPage}
                    onChange={(event) =>
                      updateTabSettings({
                        newTabPage: event.target.value === "bing" ? "bing" : "huaxia",
                      })
                    }
                  >
                    <option value="huaxia">画匣新页（默认）</option>
                    <option value="bing">Bing</option>
                  </select>
                </label>
                <label className="settings-row">
                  <span>启动时打开</span>
                  <select
                    value={tabSettings.startupMode}
                    onChange={(event) => {
                      const value = event.target.value;
                      updateTabSettings({
                        startupMode: (value === "last" || value === "specific"
                          ? value
                          : "home") as StartupMode,
                      });
                    }}
                  >
                    <option value="home">主页</option>
                    <option value="last">上次的标签</option>
                    <option value="specific">指定收藏</option>
                  </select>
                </label>
                {tabSettings.startupMode === "specific" && (
                  <>
                    <label className="settings-row">
                      <span>指定类型</span>
                      <select
                        value={tabSettings.startupKind}
                        onChange={(event) =>
                          updateTabSettings({
                            startupKind: (event.target.value === "loose"
                              ? "loose"
                              : "collection") as StartupTargetKind,
                            startupId: null,
                          })
                        }
                      >
                        <option value="collection">收藏夹</option>
                        <option value="loose">散图收藏夹</option>
                      </select>
                    </label>
                    {tabSettings.startupKind !== "loose" && (
                      <label className="settings-row">
                        <span>指定收藏夹</span>
                        <select
                          value={tabSettings.startupId ?? ""}
                          onChange={(event) =>
                            updateTabSettings({
                              startupId: event.target.value
                                ? Number(event.target.value)
                                : null,
                            })
                          }
                        >
                          <option value="">未选择（回主页）</option>
                          {collections.map((collection) => (
                            <option key={collection.id} value={collection.id}>
                              {collection.name}
                            </option>
                          ))}
                        </select>
                      </label>
                    )}
                  </>
                )}
                <label className="settings-row">
                  <span>启动时盖住内容</span>
                  <select
                    value={tabSettings.masked ? "1" : "0"}
                    onChange={(event) =>
                      updateTabSettings({ masked: event.target.value === "1" })
                    }
                  >
                    <option value="0">不盖，直接显示</option>
                    <option value="1">先盖住，点一下才显示</option>
                  </select>
                </label>
                <label className="settings-row">
                  <span>退出时彻底关闭所有标签</span>
                  <input
                    type="checkbox"
                    checked={tabSettings.clearTabsOnExit === true}
                    onChange={(event) =>
                      updateTabSettings({ clearTabsOnExit: event.target.checked })
                    }
                  />
                </label>
                <p className="settings-hint">
                  默认关闭：下次启动会原样恢复本次打开的标签（含网页和画集）。
                  勾选后，退出时清空全部标签，下次只显示主页。启动模式与指定收藏在下次启动时生效。
                </p>
              </>
            )}

            {tab === "reading" && (
              <>
                <h3>阅读偏好</h3>
                <PrefToggle
                  flagKey="huaxia.reader.showPageHud"
                  label="沉浸模式显示页码与进度线"
                  hint="关掉后沉浸阅读只剩画面本身，更彻底的沉浸（下次打开阅读生效）。"
                />
                <PrefToggle
                  flagKey="huaxia.reader.pageTransition"
                  label="翻页过渡动画（可选）"
                  hint="动效批复 A16：翻页时新页按方向轻滑入。默认关，想要再开。"
                />
                <p className="settings-hint">
                  图片管理在阅读器工具栏（图片角标按钮）和详情页都有：可增删 zip/cbz
                  的单页，删除先进回收站。
                </p>
              </>
            )}

            {tab === "wallpaper" && <WallpaperLockSettings />}
            {tab === "web" && <BrowserPrefsSettings />}

            {tab === "browser" && <BrowserDataSettings />}

            {tab === "userscript" && <UserscriptSettings />}

            {tab === "adblock" && <AdblockSettings />}

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
                {libraryOps && (
                  <div className="settings-library-ops">
                    <button
                      type="button"
                      className="btn btn-primary btn-compact"
                      disabled={libraryOps.importing}
                      onClick={libraryOps.onPickFiles}
                    >
                      导入压缩包
                    </button>
                    <button
                      type="button"
                      className="btn btn-compact"
                      disabled={libraryOps.importing}
                      onClick={libraryOps.onPickFolder}
                    >
                      导入文件夹
                    </button>
                    <button
                      type="button"
                      className="btn btn-compact"
                      disabled={libraryOps.scanning || libraryOps.importing}
                      onClick={libraryOps.onRunScan}
                    >
                      {libraryOps.scanning ? "正在同步…" : "重新扫描"}
                    </button>
                    <button
                      type="button"
                      className="btn btn-quiet btn-compact"
                      disabled={libraryOps.importing}
                      onClick={libraryOps.onSwitchLibrary}
                    >
                      切换库目录
                    </button>
                    <p className="settings-hint">导入与扫描从侧栏迁到这里，侧栏只留浏览。</p>
                  </div>
                )}
              </>
            )}

            {tab === "backup" && (
              <>
                <h3>备份与迁移</h3>
                <p className="settings-hint">
                  备份 = 库快照（VACUUM INTO 一致性副本）+ 全部设置（含壁纸锁），
                  <b>不含媒体文件本体</b>。每日自动快照：库 + 设置各保留最近 7 份。
                  恢复前会自动再做一次安全快照，怎么折腾都有后路。
                </p>
                <div className="diag-actions">
                  <button
                    type="button"
                    className="btn btn-compact"
                    disabled={backupBusy}
                    onClick={() => void runBackupExport()}
                  >
                    {backupBusy ? "备份中…" : "立即备份"}
                  </button>
                  <button
                    type="button"
                    className="btn btn-quiet btn-compact"
                    onClick={() => void refreshBackups()}
                  >
                    刷新列表
                  </button>
                </div>
                {backupError && (
                  <p className="settings-hint" style={{ color: "var(--danger)" }}>
                    {backupError}
                  </p>
                )}
                {backupReport && (
                  <p className="settings-hint" style={{ color: "var(--ok, #7fc46f)" }}>
                    {backupReport}
                  </p>
                )}
                {restoreReport && (
                  <div className="diag-item diag-ok" style={{ borderRadius: 10 }}>
                    <span className="diag-mark">✓</span>
                    <span className="diag-detail">
                      恢复完成：库{restoreReport.restored_db ? "已替换" : "未动"}（
                      {restoreReport.archive_count} 本画集）· 设置
                      {restoreReport.restored_settings ? "已替换" : "未动"}
                      {restoreReport.warnings.length > 0
                        ? ` · ${restoreReport.warnings.join("；")}`
                        : ""}
                    </span>
                  </div>
                )}
                <ul className="diag-list">
                  {(backupList ?? []).map((entry) => (
                    <li key={entry.path} className="diag-item diag-ok">
                      <span className="diag-key">
                        {KIND_LABEL[entry.kind] ?? entry.kind}
                      </span>
                      <span className="diag-detail">
                        {entry.name} · {formatBytes(entry.size)} ·{" "}
                        {new Date(entry.created_ms).toLocaleString()}
                      </span>
                      <button
                        type="button"
                        className="btn btn-quiet btn-compact"
                        disabled={restoreBusy}
                        style={{ marginLeft: "auto" }}
                        onClick={() =>
                          setRestoreConfirm(
                            restoreConfirm === entry.path ? null : entry.path,
                          )
                        }
                      >
                        {restoreConfirm === entry.path ? "收起" : "恢复"}
                      </button>
                    </li>
                  ))}
                  {backupList && backupList.length === 0 && (
                    <li className="diag-item diag-ok">
                      <span className="diag-detail">还没有备份——点「立即备份」或等每日自动快照。</span>
                    </li>
                  )}
                </ul>
                {restoreConfirm && (
                  <div className="diag-item diag-bad" style={{ borderRadius: 10, marginTop: 12 }}>
                    <span className="diag-mark">⚠</span>
                    <span className="diag-detail">
                      即将用这份备份<b>覆盖当前库与设置</b>（恢复前会自动做安全快照）。
                      确认继续吗？
                    </span>
                    <button
                      type="button"
                      className="btn btn-compact"
                      disabled={restoreBusy}
                      style={{ marginLeft: "auto" }}
                      onClick={() => void runRestore(restoreConfirm)}
                    >
                      {restoreBusy ? "恢复中…" : "确认恢复"}
                    </button>
                  </div>
                )}
              </>
            )}

            {tab === "diag" && (
              <>
                <h3>健康自检</h3>
                <p className="settings-hint">
                  一键检查：库连接 / 完整性 / 库盘可写 / 引擎加速键挂载 / 广告拦截 /
                  用户脚本 / 壁纸锁 / 日志体积。出问题时先跑这里，再导出诊断包。
                </p>
                <div className="diag-actions">
                  <button
                    type="button"
                    className="btn btn-compact"
                    disabled={healthBusy}
                    onClick={() => void runHealthCheck()}
                  >
                    {healthBusy ? "自检中…" : "跑自检"}
                  </button>
                  <button
                    type="button"
                    className="btn btn-quiet btn-compact"
                    disabled={diagBusy}
                    onClick={() => void exportDiag()}
                  >
                    {diagBusy ? "打包中…" : "导出诊断包"}
                  </button>
                </div>
                {diagError && (
                  <p className="settings-hint" style={{ color: "var(--danger)" }}>
                    {diagError}
                  </p>
                )}
                {diagBundlePath && (
                  <p className="settings-hint">诊断包已生成：{diagBundlePath}</p>
                )}
                {healthItems && (
                  <ul className="diag-list">
                    {healthItems.map((item) => (
                      <li
                        key={item.key}
                        className={`diag-item${item.ok ? " diag-ok" : " diag-bad"}`}
                      >
                        <span className="diag-mark">{item.ok ? "✓" : "✗"}</span>
                        <span className="diag-key">{item.key}</span>
                        <span className="diag-detail">{item.detail}</span>
                      </li>
                    ))}
                  </ul>
                )}
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

                <h3>从 exhentai 复制标签</h3>
                <ul className="guide-list">
                  <li>最省事：在作品页把标签区整块复制，直接粘进导入面板——画匣会自动从文本里抠出所有 tag 链接并还原成「大类 + 名字」。</li>
                  <li>
                    想要干净的纯文本：在该页面按 <code>F12</code> 打开控制台，粘贴这行回车
                    （<code>copy()</code> 是控制台专属，会把结果放进剪贴板）：
                    <code>copy([...document.querySelectorAll('a[href*="/tag/"]')].map(a =&gt; decodeURIComponent(a.href.split('/tag/')[1]).replace(/\+/g,' ')).join('\n'))</code>
                  </li>
                  <li>之后粘贴进画匣即是「female: kissing」这样的行，大类自动归位，不用一个个对照打。</li>
                </ul>

                <h3>一键抓取整部（支持 5 站）</h3>
                <ul className="guide-list">
                  <li>
                    <strong>能抓的站点</strong>：exhentai / e-hentai、nhentai.net、18comic（禁漫）、
                    wnacg（绅士漫画）。在 D2 面板「作品页网址」粘贴详情页地址 → 「开始抓取并入库」，
                    画匣会把整部图片逐张下载、打包成 CBZ 并自动入库打上「D2下载」来源标签。
                  </li>
                  <li>
                    <strong>exhentai / e-hentai</strong>：地址含 <code>/g/数字/</code>。白屏=未登录或无 exh 权限，
                    先点「登录 e-hentai」在主站登录再开 exhentai；系统代理 <code>127.0.0.1:65532</code> 要开着。
                  </li>
                  <li>
                    <strong>nhentai</strong>：地址含 <code>/g/数字/</code>，通过站内 API 取页表与标签，无需登录；
                    返回的是图片直链，逐张下载不走网页中转。
                  </li>
                  <li>
                    <strong>18comic（禁漫）</strong>：作品页地址含 <code>/album/数字/</code>。多章节作品会按章节逐页抓全；
                    部分新作品有图片混淆，若解析不出可改用 Cookie 登录后重试。
                  </li>
                  <li>
                    <strong>wnacg（绅士漫画）</strong>：作品页地址含 <code>aid-数字</code>（<code>photos-index-aid-…</code> 等）；
                    从图库页一次取全图片列表，支持 <code>wn01.cc</code> 这类数字镜像域名。
                  </li>
                  <li>
                    <strong>登录不上 / 抓取失败</strong>：D2 面板「Cookie 登录」可按站点域名粘贴浏览器 cookie
                    （exhentai / 18comic / wnacg / nhentai 四个预设）。抓取前先在浏览窗口打开作品页确认能看图，
                    再点「抓取当前页」最省事。其余站点仍可正常浏览，只是暂无解析器。
                  </li>
                  <li>
                    <strong>出口</strong>：完成后卡片可「打开库目录」；原文链接、首张图片、张数与失败数都在结果卡里。
                  </li>
                </ul>

                <h3>几个常用开关</h3>
                <ul className="guide-list">
                  <li>双击画集卡片直接开看（沉浸/普通可选）：设置 → 双击画集卡片</li>
                  <li>网页抓取登录失败：D2 面板里用「Cookie 登录」粘贴浏览器 cookie</li>
                  <li>内嵌浏览器广告拦截：D2 面板「浏览器设置」开关（改后需重开浏览窗口生效）</li>
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
                  整本抓取（旧版·按章节合并）
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


/** 阅读偏好开关：直接读写 localStorage（Reader 用 usePersistentFlag 读同一键）。 */
function PrefToggle({
  flagKey,
  label,
  hint,
}: {
  flagKey: string;
  label: string;
  hint: string;
}) {
  const [on, setOn] = useState(() => {
    try {
      return window.localStorage.getItem(flagKey) !== "0";
    } catch {
      return true;
    }
  });
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 4, margin: "8px 0" }}>
      <label style={{ display: "flex", alignItems: "center", gap: 10, cursor: "pointer" }}>
        <input
          type="checkbox"
          checked={on}
          onChange={(event) => {
            const next = event.target.checked;
            setOn(next);
            try {
              window.localStorage.setItem(flagKey, next ? "1" : "0");
            } catch {
              // 无痕模式等存不了就算了
            }
          }}
        />
        <span>{label}</span>
      </label>
      <p className="settings-hint">{hint}</p>
    </div>
  );
}
