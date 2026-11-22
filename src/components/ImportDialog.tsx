import {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useMemo,
  useRef,
  useState,
} from "react";

import { api } from "../api";
import { formatBytes } from "../format";
import { parseTagInput, summarizeByCategory } from "../tags";
import type {
  Collection,
  ImportErrorItem,
  ImportProgress,
  StageItem,
  Tag,
  TagDraft,
} from "../types";
import {
  AlertIcon,
  CheckIcon,
  CloseIcon,
  FolderIcon,
  ImportIcon,
  TagIcon,
  TrashIcon,
} from "./Icons";
import type { ToastKind } from "./Toast";

/** 点「导入」时交给上层去执行的内容。 */
export interface ImportSubmit {
  paths: string[];
  /** 全局标签：每行没单独填时用它。 */
  tags: TagDraft[];
  /** 每行单独填的标签（只带非空的行）。 */
  pathTags: { path: string; tags: TagDraft[] }[];
  collectionId: number | null;
  preserveStructure: boolean;
  /** 导入完成后把这批新画集合成一个合集（名字）；null 表示不合成。 */
  seriesName: string | null;
}

export interface ImportDialogHandle {
  /** 面板已打开时又拖进来一批：直接并入清单，不重开面板。 */
  addPaths: (paths: string[]) => void;
}

interface ImportDialogProps {
  /** 打开面板时已经确定的一批路径（来自选择器或拖放）。 */
  initialPaths: string[];
  collections: Collection[];
  /** 库里已有的标签，用来区分「新增」与「复用」。 */
  knownTags: Tag[];
  /** 窗口级拖拽悬停状态，用来高亮拖放区。 */
  dragActive: boolean;
  importing: boolean;
  progress: ImportProgress | null;
  onSubmit: (payload: ImportSubmit) => void;
  onClose: () => void;
  onPickFiles: () => void;
  onPickFolder: () => void;
  onNotify: (message: string, tone: ToastKind) => void;
}

const ImportDialog = forwardRef<ImportDialogHandle, ImportDialogProps>(
  function ImportDialog(
    {
      initialPaths,
      collections,
      knownTags,
      dragActive,
      importing,
      progress,
      onSubmit,
      onClose,
      onPickFiles,
      onPickFolder,
      onNotify,
    },
    ref,
  ) {
    const [items, setItems] = useState<StageItem[]>([]);
    const [stageErrors, setStageErrors] = useState<ImportErrorItem[]>([]);
    const [staging, setStaging] = useState(initialPaths.length > 0);
    const [tagText, setTagText] = useState("");
    const [collectionId, setCollectionId] = useState<number | "">("");
    const [preserveStructure, setPreserveStructure] = useState(false);
    /** 每行单独的标签文本（按 path 存）。空字符串＝这行用全局标签。 */
    const [rowTagText, setRowTagText] = useState<Record<string, string>>({});
    const [makeSeries, setMakeSeries] = useState(false);
    const [seriesName, setSeriesName] = useState("");

    const stage = useCallback(
      async (paths: string[]) => {
        if (paths.length === 0) return;
        setStaging(true);
        try {
          const report = await api.stageImportPaths(paths);
          setItems((previous) => {
            const seen = new Set(previous.map((item) => item.path));
            return [
              ...previous,
              ...report.items.filter((item) => !seen.has(item.path)),
            ];
          });
          if (report.errors.length > 0) {
            setStageErrors((previous) => [...previous, ...report.errors]);
          }
        } catch (error) {
          onNotify(`读取导入清单失败：${String(error)}`, "error");
        } finally {
          setStaging(false);
        }
      },
      [onNotify],
    );

    // 面板已经开着时，「选择文件」拿到的还是同一个 prop 通道，
    // 所以按内容判重而不是「只跑一次」——否则第二次选的文件永远进不来。
    const lastSeedRef = useRef<string | null>(null);

    useEffect(() => {
      if (initialPaths.length === 0) return;
      const seedKey = initialPaths.join("\n");
      if (lastSeedRef.current === seedKey) return;
      lastSeedRef.current = seedKey;
      void stage(initialPaths);
    }, [initialPaths, stage]);

    useImperativeHandle(
      ref,
      () => ({ addPaths: (paths: string[]) => void stage(paths) }),
      [stage],
    );

    const parsed = useMemo(() => parseTagInput(tagText), [tagText]);
    const groups = useMemo(
      () => summarizeByCategory(parsed.tags),
      [parsed.tags],
    );
    const knownNames = useMemo(
      () => new Set(knownTags.map((tag) => tag.name.toLowerCase())),
      [knownTags],
    );
    const draftTags = useMemo<TagDraft[]>(
      () =>
        parsed.tags.map((tag) => ({ name: tag.name, category: tag.category })),
      [parsed.tags],
    );
    const reusedTags = useMemo(
      () =>
        parsed.tags.filter((tag) => knownNames.has(tag.name.toLowerCase()))
          .length,
      [parsed.tags, knownNames],
    );

    const importable = useMemo(
      () => items.filter((item) => !item.issue),
      [items],
    );
    const duplicateCount = importable.filter((item) => item.duplicate).length;
    const totalBytes = importable.reduce(
      (sum, item) => sum + Math.max(0, item.sizeBytes),
      0,
    );
    const knownPages = importable.filter((item) => item.pageCount !== null);
    const pageTotal = knownPages.reduce(
      (sum, item) => sum + (item.pageCount ?? 0),
      0,
    );

    // 类型识别：压缩包走画廊，图片走散图，其余原样提示（阶段报告里会有 issue）。
    const ARCHIVE_EXTS = new Set(["zip", "cbz", "rar", "7z", "cbr"]);
    const IMAGE_EXTS = new Set(["jpg", "jpeg", "png", "webp", "gif", "avif", "bmp"]);
    const extOf = (item: StageItem) => item.extension.toLowerCase().replace(/^[.]/, "");
    const archives = importable.filter((item) => ARCHIVE_EXTS.has(extOf(item)));
    const images = importable.filter((item) => IMAGE_EXTS.has(extOf(item)));
    const others = importable.length - archives.length - images.length;

    const busy = staging || importing;
    const percent =
      progress && progress.total > 0
        ? Math.min(100, (progress.current / progress.total) * 100)
        : 0;

    const removeItem = useCallback((path: string) => {
      setItems((previous) => previous.filter((item) => item.path !== path));
      // 行标签也一并清掉，避免换一批同名路径时残留上一次的输入。
      setRowTagText((previous) => {
        if (!(path in previous)) return previous;
        const next = { ...previous };
        delete next[path];
        return next;
      });
    }, []);

    const submit = useCallback(() => {
      if (busy || importable.length === 0) return;
      // 每行单独填的标签：只把填了内容的行传上去，空行由后端回落到全局标签。
      const pathTags = importable
        .map((item) => {
          const text = (rowTagText[item.path] ?? "").trim();
          if (!text) return null;
          const rowParsed = parseTagInput(text);
          return {
            path: item.path,
            tags: rowParsed.tags.map((tag) => ({
              name: tag.name,
              category: tag.category,
            })),
          };
        })
        .filter((entry): entry is { path: string; tags: TagDraft[] } => entry !== null);

      onSubmit({
        paths: importable.map((item) => item.path),
        tags: draftTags,
        pathTags,
        collectionId: collectionId === "" ? null : collectionId,
        preserveStructure,
        seriesName:
          makeSeries && archives.length >= 2
            ? seriesName.trim() || archives[0].stem
            : null,
      });
    }, [
      busy,
      importable,
      onSubmit,
      draftTags,
      collectionId,
      preserveStructure,
      rowTagText,
      makeSeries,
      seriesName,
      archives,
    ]);

    const requestClose = useCallback(() => {
      // 导入过程中关掉面板会让用户误以为任务被取消了，直接不给关。
      if (importing) {
        onNotify("正在导入，完成前请不要关闭面板", "info");
        return;
      }
      onClose();
    }, [importing, onClose, onNotify]);

    useEffect(() => {
      const onKeyDown = (event: KeyboardEvent) => {
        if (event.key !== "Escape") return;
        const target = event.target as HTMLElement | null;
        // 焦点在标签输入框里时，Esc 只退出输入，避免一句话没粘贴完就把面板关了。
        if (target?.tagName === "TEXTAREA") {
          target.blur();
          return;
        }
        requestClose();
      };
      window.addEventListener("keydown", onKeyDown);
      return () => window.removeEventListener("keydown", onKeyDown);
    }, [requestClose]);

    return (
      <div className="modal-backdrop" onClick={requestClose}>
        <section
          className={`modal import-dialog${dragActive ? " is-drop-target" : ""}`}
          role="dialog"
          aria-modal="true"
          aria-label="导入新画廊"
          onClick={(event) => event.stopPropagation()}
        >
          <div className="modal-title-row">
            <div>
              <p className="eyebrow">IMPORT</p>
              <h2>导入新画廊</h2>
            </div>
            <button
              type="button"
              className="icon-button"
              onClick={requestClose}
              aria-label="关闭"
            >
              <CloseIcon />
            </button>
          </div>

          <div className="import-dialog-body">
            <section className="import-pane">
              <div className="import-pane-head">
                <h3>
                  待导入
                  {items.length > 0 && (
                    <span className="import-count">{items.length}</span>
                  )}
                </h3>
                <div className="import-pane-tools">
                  <button
                    type="button"
                    className="btn btn-quiet btn-compact"
                    onClick={onPickFiles}
                    disabled={busy}
                  >
                    <ImportIcon width={12} height={12} />
                    选择文件
                  </button>
                  <button
                    type="button"
                    className="btn btn-quiet btn-compact"
                    onClick={onPickFolder}
                    disabled={busy}
                  >
                    <FolderIcon width={12} height={12} />
                    选择文件夹
                  </button>
                  {items.length > 0 && (
                    <button
                      type="button"
                      className="btn btn-quiet btn-compact"
                      onClick={() => {
                        setItems([]);
                        setStageErrors([]);
                      }}
                      disabled={busy}
                    >
                      <TrashIcon width={12} height={12} />
                      清空
                    </button>
                  )}
                </div>
              </div>

              {items.length === 0 ? (
                <button
                  type="button"
                  className={`import-dropzone${dragActive ? " is-active" : ""}`}
                  onClick={onPickFiles}
                  disabled={busy}
                >
                  <span className="import-dropzone-icon">
                    <ImportIcon width={26} height={26} />
                  </span>
                  <strong>
                    {staging ? "正在读取清单…" : "把 ZIP / CBZ 拖进窗口"}
                  </strong>
                  <span>
                    也可以拖到窗口任意位置；或点这里选择文件、文件夹。
                  </span>
                  <small>画集会复制进库目录，源文件保持不动。</small>
                </button>
              ) : (
                <>
                  <div className="import-type-summary">
                    {archives.length > 0 && (
                      <span className="import-type-chip">
                        压缩包 {archives.length} · 画廊
                      </span>
                    )}
                    {images.length > 0 && (
                      <span className="import-type-chip is-image">
                        图片 {images.length} · 散图（走散图收藏夹）
                      </span>
                    )}
                    {others > 0 && (
                      <span className="import-type-chip is-other">
                        其他 {others} · 可能无法导入
                      </span>
                    )}
                  </div>
                <ul className="import-file-list">
                  {items.map((item) => (
                    <li
                      key={item.path}
                      className={`import-file${item.issue ? " is-bad" : ""}`}
                      title={item.path}
                    >
                      <div className="import-file-main">
                        <div className="import-file-title">
                          <strong>{item.stem}</strong>
                          <span className="import-file-ext">
                            {item.extension.toUpperCase() || "文件"}
                          </span>
                        </div>
                        <div className="import-file-meta">
                          <span>{formatBytes(item.sizeBytes)}</span>
                          {item.pageCount !== null && (
                            <>
                              <i />
                              <span>{item.pageCount} 页</span>
                            </>
                          )}
                          {item.duplicate && (
                            <span className="import-badge is-dup">
                              疑似重复
                            </span>
                          )}
                          {item.inLibrary && (
                            <span className="import-badge">库内文件</span>
                          )}
                          {item.issue && (
                            <span className="import-badge is-bad">
                              {item.issue}
                            </span>
                          )}
                        </div>
                        {!item.issue && (
                          <input
                            className="import-row-tags"
                            value={rowTagText[item.path] ?? ""}
                            placeholder="这本单独的标签（留空＝用右侧那套）"
                            disabled={busy}
                            onChange={(event) =>
                              setRowTagText((previous) => ({
                                ...previous,
                                [item.path]: event.target.value,
                              }))
                            }
                          />
                        )}
                      </div>
                      <button
                        type="button"
                        className="icon-button import-file-remove"
                        onClick={() => removeItem(item.path)}
                        disabled={busy}
                        aria-label={`移除 ${item.fileName}`}
                      >
                        <CloseIcon width={13} height={13} />
                      </button>
                    </li>
                  ))}
                </ul>
                </>
              )}

              {stageErrors.length > 0 && (
                <ul className="import-stage-errors">
                  {stageErrors.map((error, index) => (
                    <li key={`${error.file}-${index}`}>
                      <AlertIcon width={12} height={12} />
                      <span title={error.file}>{error.message}</span>
                    </li>
                  ))}
                </ul>
              )}
            </section>

            <section className="import-pane import-pane-side">
              <div className="import-pane-head">
                <h3>
                  <TagIcon width={13} height={13} />
                  标签
                </h3>
                <span className="import-pane-note">
                  {parsed.tags.length === 0 ? "可选" : `${parsed.tags.length} 个`}
                </span>
              </div>

              <textarea
                className="import-tag-input"
                rows={6}
                value={tagText}
                disabled={busy}
                placeholder={
                  "粘贴标准 tag 格式，命名空间会自动映射到大类：\nfemale:big breasts, sole female\ncharacter:aoba utsumihikari\nartist:mountainhan\n\n也支持每行一个或逗号分隔的纯标签。"
                }
                onChange={(event) => setTagText(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) {
                    event.preventDefault();
                    submit();
                  }
                }}
              />

              {groups.length > 0 && (
                <div className="tag-bulk-groups">
                  {groups.map(([category, count]) => (
                    <span className="tag-bulk-group" key={category}>
                      {category}
                      <b>{count}</b>
                    </span>
                  ))}
                </div>
              )}

              {parsed.rejected.length > 0 && (
                <p className="tag-bulk-warn">
                  <AlertIcon width={12} height={12} />
                  <span>
                    {parsed.rejected
                      .map((item) => `${item.name}：${item.reason}`)
                      .join("；")}
                  </span>
                </p>
              )}

              <p className="import-tag-hint">
                {parsed.tags.length === 0
                  ? "留空则只导入画集，不挂标签。"
                  : `新增 ${parsed.tags.length - reusedTags} 个 · 复用已有 ${reusedTags} 个，导入后挂到每一本上。`}
              </p>

              <div className="import-options">
                <label className="import-option">
                  <span>加入收藏夹</span>
                  <span className="select-field">
                    <select
                      value={collectionId}
                      disabled={busy}
                      onChange={(event) =>
                        setCollectionId(
                          event.target.value === ""
                            ? ""
                            : Number(event.target.value),
                        )
                      }
                    >
                      <option value="">不加入</option>
                      {collections.map((collection) => (
                        <option key={collection.id} value={collection.id}>
                          {collection.name}
                        </option>
                      ))}
                    </select>
                  </span>
                </label>

                <label className="import-option is-check">
                  <input
                    type="checkbox"
                    checked={preserveStructure}
                    disabled={busy}
                    onChange={(event) =>
                      setPreserveStructure(event.target.checked)
                    }
                  />
                  <span>
                    保留文件夹结构
                    <em>按源目录层级落库，适合按作者/系列整理的压缩包</em>
                  </span>
                </label>
              </div>
            </section>
          </div>

          <footer className="import-dialog-foot">
            {!importing && archives.length >= 2 && (
              <div className="import-series-box">
                <label className="import-series-toggle">
                  <input
                    type="checkbox"
                    checked={makeSeries}
                    disabled={busy}
                    onChange={(event) =>
                      setMakeSeries(event.target.checked)
                    }
                  />
                  导入后按上面的顺序合成合集（主网格只留一张轮换封面的卡）
                </label>
                {makeSeries && (
                  <input
                    className="import-series-name"
                    value={seriesName}
                    placeholder={`合集名（默认「${archives[0].stem}」）`}
                    disabled={busy}
                    onChange={(event) => setSeriesName(event.target.value)}
                  />
                )}
              </div>
            )}
            {importing ? (
              <div className="import-progress">
                <div className="import-progress-text">
                  <span>{progress?.fileName || "正在准备…"}</span>
                  <span>
                    {progress?.current ?? 0} / {progress?.total ?? 0}
                  </span>
                </div>
                <div
                  className={`progress-track${(progress?.total ?? 0) === 0 ? " is-indeterminate" : ""}`}
                >
                  <div
                    className="progress-fill"
                    style={
                      (progress?.total ?? 0) > 0
                        ? { width: `${percent}%` }
                        : undefined
                    }
                  />
                </div>
                <div className="import-progress-stats">
                  <span>
                    已导入 <b>{progress?.imported ?? 0}</b>
                  </span>
                  <span>
                    跳过 <b>{progress?.skipped ?? 0}</b>
                  </span>
                  <span>
                    失败 <b>{progress?.failed ?? 0}</b>
                  </span>
                </div>
              </div>
            ) : (
              <div className="import-summary">
                <strong>
                  {importable.length === 0
                    ? "还没有可导入的画集"
                    : `共 ${importable.length} 本 · ${formatBytes(totalBytes)}`}
                </strong>
                <span>
                  {importable.length > 0 && knownPages.length > 0 && (
                    <>
                      约 {pageTotal} 页 ·{" "}
                    </>
                  )}
                  {duplicateCount > 0
                    ? draftTags.length > 0
                      ? `${duplicateCount} 本已在库中：不复制，只补标签`
                      : `${duplicateCount} 本疑似重复，导入时跳过`
                    : "压缩包会复制进库目录"}
                </span>
              </div>
            )}

            <div className="import-dialog-actions">
              <button
                type="button"
                className="btn btn-ghost"
                onClick={requestClose}
                disabled={importing}
              >
                取消
              </button>
              <button
                type="button"
                className="btn btn-primary"
                onClick={submit}
                disabled={busy || importable.length === 0}
              >
                {importing ? (
                  "导入中…"
                ) : staging ? (
                  "读取中…"
                ) : (
                  <>
                    <CheckIcon width={13} height={13} />
                    {importable.length === 0
                      ? "导入"
                      : `导入 ${importable.length} 本`}
                  </>
                )}
              </button>
            </div>
          </footer>
        </section>
      </div>
    );
  },
);

export default ImportDialog;
