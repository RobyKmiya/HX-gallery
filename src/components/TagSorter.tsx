import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  parseExHentaiTags,
  type ParseOptions,
  type ParseResult,
} from "../exhentaiTagParser";
import { NAMESPACE_CATEGORIES } from "../tags";
import type { BulkTagReport, TagDraft } from "../types";
import { AlertIcon, CheckIcon, CloseIcon, RefreshIcon, TagIcon, TrashIcon } from "./Icons";

interface TagSorterProps {
  onClose: () => void;
  onNotify: (message: string, kind?: "info" | "success" | "error") => void;
  /** 从画集详情页拉起时带上画集信息，界面上会显示关联对象。 */
  archive?: { id: number; title: string };
  /** 用户点「确认导入」后才会被调用；不传则隐藏导入功能（纯整理模式）。 */
  onImport?: (drafts: TagDraft[]) => Promise<BulkTagReport>;
}

interface DraftTag {
  id: string;
  text: string;
  picked: boolean;
}

interface DraftBlock {
  key: string;
  included: boolean;
  tags: DraftTag[];
}

const SEPARATOR_OPTIONS: { value: string; label: string }[] = [
  { value: "，", label: "中文逗号 ，" },
  { value: ", ", label: "英文逗号 , " },
  { value: "、", label: "顿号 、" },
];

const BLOCK_SEPARATOR_OPTIONS: { value: string; label: string }[] = [
  { value: "\n\n", label: "分类间空一行" },
  { value: "\n", label: "分类间不空行" },
];

const PLACEHOLDER = `在这里粘贴 ExHentai 的标签表格（Markdown / 网页纯文本 / HTML 源码都行）：

| parody: | [**arknights**](https://exhentai.org/tag/parody:arknights) | [**genshin impact**](https://exhentai.org/tag/parody:genshin+impact) |
| female: | [**big breasts**](https://exhentai.org/tag/female:big+breasts) | [**blowjob**](https://exhentai.org/tag/female:blowjob) |

也可以直接贴：
female: big breasts, sole female
artist: mountainhan

只有标签名、彼此之间全靠空格的那种（arknights genshin impact …）分不出边界，
请换成带链接或逗号的格式。`;

/** 画集导入时把 ExHentai 命名空间映射成应用内大类（未收录的归 null）。 */
function toAppCategory(key: string): string | null {
  return NAMESPACE_CATEGORIES[key.toLowerCase()] ?? null;
}

function toDraftBlocks(result: ParseResult, nextId: () => string): DraftBlock[] {
  return result.blocks.map((block) => ({
    key: block.key,
    included: true,
    tags: block.tags.map((text) => ({ id: nextId(), text, picked: false })),
  }));
}

/**
 * 写剪贴板。
 *
 * 先走 `navigator.clipboard`（与 CopyTitleButton 一致）；它在窗口失焦或权限被拒时
 * 会抛 NotAllowedError，而「复制」正是这个功能的交付物，不能静默失败 —— 所以补一条
 * `execCommand` 的兜底路径（临时 textarea + select），WebView2 下可用。
 */
async function writeClipboard(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    // 落到兜底
  }
  try {
    const helper = document.createElement("textarea");
    helper.value = text;
    helper.setAttribute("readonly", "");
    helper.style.position = "fixed";
    helper.style.top = "-1000px";
    helper.style.opacity = "0";
    document.body.appendChild(helper);
    helper.select();
    const done = document.execCommand("copy");
    document.body.removeChild(helper);
    return done;
  } catch {
    return false;
  }
}

/**
 * ExHentai 标签整理器。
 *
 * 四步闭环：粘贴 → 解析 → 整理 → 复制。
 *
 * 状态分层（这是刻意的）：
 * - `source` 是用户粘贴的原文，只读不改；
 * - `parsed` 是解析器上一次的产出，作为「原始结果」留存；
 * - `blocks` 是可编辑草稿。人工改动只落在 `blocks` 上，`parsed` 永远干净，
 *   所以「重新解析」随时能把草稿推倒重来，也不会污染最初提取的数据。
 *
 * 不做的事：不写库、不覆盖画集原有标签、不发网络请求。唯一的写操作是用户
 * 在关联画集后点两次「确认导入」。
 */
export default function TagSorter({
  onClose,
  onNotify,
  archive,
  onImport,
}: TagSorterProps) {
  const [source, setSource] = useState("");
  const [parsed, setParsed] = useState<ParseResult | null>(null);
  const [blocks, setBlocks] = useState<DraftBlock[]>([]);
  const [touched, setTouched] = useState(false);
  const [confirmReset, setConfirmReset] = useState(false);
  const [confirmImport, setConfirmImport] = useState(false);
  const [importing, setImporting] = useState(false);
  const [dedupe, setDedupe] = useState(true);
  const [tagSeparator, setTagSeparator] = useState("，");
  const [blockSeparator, setBlockSeparator] = useState("\n\n");
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editingValue, setEditingValue] = useState("");
  const [copied, setCopied] = useState<"all" | "picked" | string | null>(null);
  const [lastReport, setLastReport] = useState<BulkTagReport | null>(null);

  const uidRef = useRef(0);
  const copyTimerRef = useRef<number | null>(null);
  const editInputRef = useRef<HTMLInputElement | null>(null);
  /** Esc 取消编辑时置真，压掉随之而来的 blur 提交，免得「取消」又把手改写回去。 */
  const editCancelledRef = useRef(false);

  const nextId = useCallback(() => {
    uidRef.current += 1;
    return `t${uidRef.current}`;
  }, []);

  useEffect(() => {
    return () => {
      if (copyTimerRef.current !== null) window.clearTimeout(copyTimerRef.current);
    };
  }, []);

  useEffect(() => {
    if (editingId && editInputRef.current) {
      editInputRef.current.focus();
      editInputRef.current.select();
    }
  }, [editingId]);

  const options = useMemo<ParseOptions>(
    () => ({ dedupe, tagSeparator, blockSeparator }),
    [dedupe, tagSeparator, blockSeparator],
  );

  const runParse = useCallback(
    (text: string, opts: ParseOptions) => {
      const result = parseExHentaiTags(text, opts);
      setParsed(result);
      setBlocks(toDraftBlocks(result, nextId));
      setTouched(false);
      setConfirmReset(false);
      setConfirmImport(false);
      setLastReport(null);
      if (result.success) {
        onNotify(
          `识别到 ${result.statistics.categoryCount} 类 · ${result.statistics.tagCount} 个标签`,
          "success",
        );
      } else if (result.errors[0]) {
        onNotify(result.errors[0].message, "error");
      }
    },
    [nextId, onNotify],
  );

  const handleParse = useCallback(() => {
    if (!source.trim()) {
      onNotify("还没有粘贴内容", "info");
      return;
    }
    runParse(source, options);
  }, [source, options, runParse, onNotify]);

  const handleReset = useCallback(() => {
    if (!source.trim()) return;
    if (touched && !confirmReset) {
      setConfirmReset(true);
      onNotify("再点一次会丢弃手改，回到最初的解析结果", "info");
      return;
    }
    runParse(source, options);
  }, [source, options, touched, confirmReset, runParse, onNotify]);

  const handleClear = useCallback(() => {
    setSource("");
    setParsed(null);
    setBlocks([]);
    setTouched(false);
    setConfirmReset(false);
    setConfirmImport(false);
    setLastReport(null);
    setEditingId(null);
  }, []);

  const includedBlocks = useMemo(
    () => blocks.filter((block) => block.included && block.tags.length > 0),
    [blocks],
  );

  const outputText = useMemo(
    () =>
      includedBlocks
        .map((block) => `${block.key}: ${block.tags.map((tag) => tag.text).join(tagSeparator)}`)
        .join(blockSeparator),
    [includedBlocks, tagSeparator, blockSeparator],
  );

  const statistics = useMemo(() => {
    const tagCount = includedBlocks.reduce((sum, block) => sum + block.tags.length, 0);
    return {
      categoryCount: includedBlocks.length,
      tagCount,
      duplicatesRemoved: parsed?.statistics.duplicatesRemoved ?? 0,
      emptyCells: parsed?.statistics.emptyCells ?? 0,
      overlong: parsed?.statistics.overlong ?? 0,
      pickedCount: blocks.reduce(
        (sum, block) => sum + block.tags.filter((tag) => tag.picked).length,
        0,
      ),
    };
  }, [includedBlocks, blocks, parsed]);

  const copyText = useCallback(
    async (text: string, mark: string) => {
      if (!text.trim()) {
        onNotify("没有可复制的内容", "info");
        return;
      }
      const done = await writeClipboard(text);
      if (!done) {
        onNotify("复制失败，剪贴板被拒绝", "error");
        return;
      }
      setCopied(mark);
      if (copyTimerRef.current !== null) window.clearTimeout(copyTimerRef.current);
      copyTimerRef.current = window.setTimeout(() => setCopied(null), 1400);
    },
    [onNotify],
  );

  const pickedText = useMemo(() => {
    const lines: string[] = [];
    for (const block of blocks) {
      if (!block.included) continue;
      const picked = block.tags.filter((tag) => tag.picked);
      if (picked.length === 0) continue;
      lines.push(`${block.key}: ${picked.map((tag) => tag.text).join(tagSeparator)}`);
    }
    return lines.join(blockSeparator);
  }, [blocks, tagSeparator, blockSeparator]);

  const toggleBlock = (key: string) => {
    setBlocks((previous) =>
      previous.map((block) =>
        block.key === key ? { ...block, included: !block.included } : block,
      ),
    );
  };

  const removeBlock = (key: string) => {
    setBlocks((previous) => previous.filter((block) => block.key !== key));
    setTouched(true);
  };

  const removeTag = (blockKey: string, tagId: string) => {
    setBlocks((previous) =>
      previous
        .map((block) =>
          block.key === blockKey
            ? { ...block, tags: block.tags.filter((tag) => tag.id !== tagId) }
            : block,
        )
        .filter((block) => block.tags.length > 0),
    );
    setTouched(true);
  };

  const toggleTag = (blockKey: string, tagId: string) => {
    setBlocks((previous) =>
      previous.map((block) =>
        block.key === blockKey
          ? {
              ...block,
              tags: block.tags.map((tag) =>
                tag.id === tagId ? { ...tag, picked: !tag.picked } : tag,
              ),
            }
          : block,
      ),
    );
  };

  const startEdit = (tag: DraftTag) => {
    editCancelledRef.current = false;
    setEditingId(tag.id);
    setEditingValue(tag.text);
  };

  const cancelEdit = () => {
    editCancelledRef.current = true;
    setEditingId(null);
  };

  const commitEdit = () => {
    if (!editingId) return;
    if (editCancelledRef.current) {
      editCancelledRef.current = false;
      setEditingId(null);
      return;
    }
    const value = editingValue.trim();
    const targetId = editingId;
    setEditingId(null);
    if (!value) {
      setBlocks((previous) =>
        previous
          .map((block) => ({
            ...block,
            tags: block.tags.filter((tag) => tag.id !== targetId),
          }))
          .filter((block) => block.tags.length > 0),
      );
      setTouched(true);
      return;
    }
    setBlocks((previous) =>
      previous.map((block) => ({
        ...block,
        tags: block.tags.map((tag) => (tag.id === targetId ? { ...tag, text: value } : tag)),
      })),
    );
    setTouched(true);
  };

  const handleImport = useCallback(async () => {
    if (!onImport || !archive) return;
    const drafts: TagDraft[] = includedBlocks.flatMap((block) =>
      block.tags.map((tag) => ({
        name: tag.text,
        category: toAppCategory(block.key),
      })),
    );
    if (drafts.length === 0) {
      onNotify("没有可导入的标签", "info");
      return;
    }
    if (!confirmImport) {
      setConfirmImport(true);
      onNotify(`再点一次确认：把 ${drafts.length} 个标签写进「${archive.title}」`, "info");
      return;
    }

    setImporting(true);
    try {
      const report = await onImport(drafts);
      setLastReport(report);
      setConfirmImport(false);
      const parts = [`已导入 ${report.linked} 个`];
      if (report.added > 0) parts.push(`新建 ${report.added}`);
      if (report.categorized > 0) parts.push(`补分类 ${report.categorized}`);
      if (report.failed.length > 0) {
        onNotify(
          `${parts.join(" · ")}；${report.failed.length} 个失败：${report.failed[0].name}（${report.failed[0].reason}）`,
          "error",
        );
      } else {
        onNotify(parts.join(" · "), "success");
      }
    } catch (error) {
      onNotify(`导入失败：${String(error)}`, "error");
    } finally {
      setImporting(false);
    }
  }, [onImport, archive, includedBlocks, confirmImport, onNotify]);

  const errors = parsed?.errors ?? [];

  return (
    <div className="modal-backdrop tag-sorter-backdrop" role="presentation" onClick={onClose}>
      <div
        className="tag-sorter"
        role="dialog"
        aria-modal="true"
        aria-label="ExHentai 标签整理器"
        onClick={(event) => event.stopPropagation()}
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            event.stopPropagation();
            if (editingId) {
              cancelEdit();
              return;
            }
            onClose();
          }
        }}
      >
        <header className="tag-sorter-head">
          <div className="tag-sorter-title">
            <span className="tag-sorter-mark">
              <TagIcon width={19} height={19} />
            </span>
            <div>
              <p className="eyebrow">TAG SORTER</p>
              <h2>ExHentai 标签整理器</h2>
              <p>
                {archive
                  ? `关联画集：${archive.title}`
                  : "粘贴表格 → 解析 → 整理 → 复制；全程本地，不联网"}
              </p>
            </div>
          </div>
          <div className="tag-sorter-head-actions">
            <button
              type="button"
              className="icon-button"
              onClick={handleClear}
              title="清空输入与结果"
              aria-label="清空"
            >
              <TrashIcon width={15} height={15} />
            </button>
            <button
              type="button"
              className="icon-button"
              onClick={onClose}
              title="关闭"
              aria-label="关闭标签整理器"
            >
              <CloseIcon width={16} height={16} />
            </button>
          </div>
        </header>

        <div className="tag-sorter-body">
          <div className="tag-sorter-col">
            <section className="tag-sorter-pane tag-sorter-pane-grow">
              <div className="tag-sorter-pane-head">
                <span>
                  <b>①</b> 原始数据
                </span>
                <small>Markdown / 网页复制 / 纯文本</small>
              </div>
              <textarea
                className="tag-sorter-textarea"
                value={source}
                spellCheck={false}
                placeholder={PLACEHOLDER}
                onChange={(event) => setSource(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) {
                    event.preventDefault();
                    handleParse();
                  }
                }}
              />
              <div className="tag-sorter-pane-foot">
                <button
                  type="button"
                  className="btn btn-primary btn-compact"
                  onClick={handleParse}
                  title="Ctrl + Enter 也能解析"
                >
                  <RefreshIcon width={14} height={14} />
                  解析标签
                </button>
                <label className="tag-sorter-check" title="同一分类里重复出现的标签只保留第一个">
                  <input
                    type="checkbox"
                    checked={dedupe}
                    onChange={(event) => setDedupe(event.target.checked)}
                  />
                  同分类去重
                </label>
              </div>
            </section>

            <section className="tag-sorter-pane tag-sorter-pane-grow">
              <div className="tag-sorter-pane-head">
                <span>
                  <b>③</b> 输出结果
                </span>
                <select
                  className="tag-sorter-select"
                  value={tagSeparator}
                  onChange={(event) => setTagSeparator(event.target.value)}
                  title="标签之间的分隔符"
                >
                  {SEPARATOR_OPTIONS.map((option) => (
                    <option key={option.value} value={option.value}>
                      {option.label}
                    </option>
                  ))}
                </select>
                <select
                  className="tag-sorter-select"
                  value={blockSeparator}
                  onChange={(event) => setBlockSeparator(event.target.value)}
                  title="分类之间的分隔方式"
                >
                  {BLOCK_SEPARATOR_OPTIONS.map((option) => (
                    <option key={option.value} value={option.value}>
                      {option.label}
                    </option>
                  ))}
                </select>
              </div>
              <textarea
                className="tag-sorter-textarea tag-sorter-output"
                value={outputText}
                readOnly
                spellCheck={false}
                placeholder="解析之后这里出现可直接复制的纯文本"
              />
              <div className="tag-sorter-pane-foot">
                <button
                  type="button"
                  className="btn btn-primary btn-compact"
                  disabled={!outputText}
                  onClick={() => void copyText(outputText, "all")}
                >
                  {copied === "all" ? (
                    <CheckIcon width={13} height={13} />
                  ) : (
                    <CopyGlyph />
                  )}
                  {copied === "all" ? "已复制" : "复制全部"}
                </button>
                <button
                  type="button"
                  className="btn btn-ghost btn-compact"
                  disabled={statistics.pickedCount === 0}
                  onClick={() => void copyText(pickedText, "picked")}
                  title="先在中间点选几个标签"
                >
                  {copied === "picked" ? "已复制" : `复制选中 ${statistics.pickedCount || ""}`}
                </button>
                {archive && onImport && (
                  <button
                    type="button"
                    className={`btn btn-compact ${confirmImport ? "btn-danger" : "btn-ghost"}`}
                    disabled={importing || includedBlocks.length === 0}
                    onClick={() => void handleImport()}
                    title="会写进这本画集的标签，原标签不会被覆盖"
                  >
                    {importing
                      ? "导入中…"
                      : confirmImport
                        ? `确认导入 ${statistics.tagCount} 个`
                        : "导入到本画集"}
                  </button>
                )}
              </div>
            </section>
          </div>

          <section className="tag-sorter-pane tag-sorter-result">
            <div className="tag-sorter-pane-head">
              <span>
                <b>②</b> 解析结果
              </span>
              <small>
                {statistics.categoryCount} 类 · {statistics.tagCount} 个标签
                {statistics.duplicatesRemoved > 0
                  ? ` · 去重 ${statistics.duplicatesRemoved}`
                  : ""}
                {statistics.emptyCells > 0 ? ` · 空单元格 ${statistics.emptyCells}` : ""}
              </small>
              <button
                type="button"
                className={`btn btn-quiet btn-compact${confirmReset ? " is-danger" : ""}`}
                onClick={handleReset}
                disabled={!source.trim()}
                title="丢掉手改，按当前原文与选项重新解析"
              >
                {confirmReset ? "确认重解析" : "重新解析"}
              </button>
            </div>

            {blocks.length === 0 ? (
              <p className="tag-sorter-empty">
                {parsed ? "上一次解析没有结果" : "左侧粘贴表格后点「解析标签」"}
              </p>
            ) : (
              <div className="tag-sorter-groups">
                {blocks.map((block) => (
                  <article
                    className={`tag-sorter-group${block.included ? "" : " is-off"}`}
                    key={block.key}
                  >
                    <header className="tag-sorter-group-head">
                      <label className="tag-sorter-check">
                        <input
                          type="checkbox"
                          checked={block.included}
                          onChange={() => toggleBlock(block.key)}
                        />
                        <strong>{block.key}</strong>
                      </label>
                      <span className="tag-sorter-group-count">{block.tags.length}</span>
                      <div className="tag-sorter-group-actions">
                        <button
                          type="button"
                          className="icon-button"
                          onClick={() =>
                            void copyText(
                              `${block.key}: ${block.tags.map((tag) => tag.text).join(tagSeparator)}`,
                              block.key,
                            )
                          }
                          title={`复制「${block.key}」这一类`}
                          aria-label={`复制 ${block.key}`}
                        >
                          {copied === block.key ? (
                            <CheckIcon width={12} height={12} />
                          ) : (
                            <CopyGlyph width={12} height={12} />
                          )}
                        </button>
                        <button
                          type="button"
                          className="icon-button"
                          onClick={() => removeBlock(block.key)}
                          title={`删除「${block.key}」整类`}
                          aria-label={`删除 ${block.key}`}
                        >
                          <CloseIcon width={12} height={12} />
                        </button>
                      </div>
                    </header>
                    <div className="tag-sorter-tags">
                      {block.tags.map((tag) =>
                        editingId === tag.id ? (
                          <input
                            key={tag.id}
                            ref={editInputRef}
                            className="tag-sorter-edit"
                            value={editingValue}
                            onChange={(event) => setEditingValue(event.target.value)}
                            onBlur={commitEdit}
                            onKeyDown={(event) => {
                              if (event.key === "Enter" && !event.nativeEvent.isComposing) {
                                event.preventDefault();
                                commitEdit();
                              }
                              if (event.key === "Escape") {
                                event.stopPropagation();
                                cancelEdit();
                              }
                            }}
                          />
                        ) : (
                          <span
                            key={tag.id}
                            className={`tag-sorter-chip${tag.picked ? " is-picked" : ""}`}
                          >
                            <button
                              type="button"
                              className="tag-sorter-chip-text"
                              onClick={() => toggleTag(block.key, tag.id)}
                              title={tag.picked ? "点击取消选中" : "点击选中（可单独复制）"}
                            >
                              {tag.text}
                            </button>
                            <button
                              type="button"
                              className="tag-sorter-chip-btn"
                              onClick={() => startEdit(tag)}
                              title="改这个标签"
                              aria-label={`编辑 ${tag.text}`}
                            >
                              <PencilGlyph />
                            </button>
                            <button
                              type="button"
                              className="tag-sorter-chip-btn"
                              onClick={() => removeTag(block.key, tag.id)}
                              title="删掉这个标签"
                              aria-label={`删除 ${tag.text}`}
                            >
                              <CloseIcon width={10} height={10} />
                            </button>
                          </span>
                        ),
                      )}
                    </div>
                  </article>
                ))}
              </div>
            )}

            {errors.length > 0 && (
              <div className="tag-sorter-errors">
                {errors.slice(0, 4).map((error, index) => (
                  <p className="tag-bulk-warn" key={`${error.code}-${index}`}>
                    <AlertIcon width={12} height={12} />
                    {error.line ? `第 ${error.line} 行：` : ""}
                    {error.message}
                  </p>
                ))}
                {errors.length > 4 && (
                  <p className="tag-sorter-hint">另有 {errors.length - 4} 条提示未显示</p>
                )}
              </div>
            )}

            {lastReport && (
              <p className="tag-sorter-hint">
                上次导入：成功 {lastReport.linked} · 新建 {lastReport.added} · 补分类{" "}
                {lastReport.categorized} · 失败 {lastReport.failed.length}
              </p>
            )}

            <p className="tag-sorter-hint">
              分隔依据只有表格单元格和链接边界，空格不会拆开标签；
              <code>big breasts</code>、<code>genshin impact</code> 都是一个完整标签。
            </p>
          </section>
        </div>
      </div>
    </div>
  );
}

function CopyGlyph({ width = 13, height = 13 }: { width?: number; height?: number }) {
  return (
    <svg
      width={width}
      height={height}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.8}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <rect x="9" y="9" width="11" height="11" rx="2" />
      <path d="M6 15V5.5A1.5 1.5 0 0 1 7.5 4H15" />
    </svg>
  );
}

function PencilGlyph() {
  return (
    <svg
      width={11}
      height={11}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.9}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M4 20h4L19 9a2.5 2.5 0 0 0-3.5-3.5L4 16v4z" />
    </svg>
  );
}
