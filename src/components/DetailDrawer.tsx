import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { api, assetUrl } from "../api";
import { formatBytes, formatDateTime } from "../format";
import { parseTagInput, summarizeByCategory } from "../tags";
import type {
  ArchiveCardData,
  BulkTagReport,
  Collection,
  Queue,
  Tag,
} from "../types";
import {
  AlertIcon,
  BookIcon,
  CheckIcon,
  CloseIcon,
  HeartIcon,
  LogoIcon,
  PlusIcon,
  RevealIcon,
  SearchIcon,
  StarIcon,
  SwapIcon,
  TagIcon,
  TrashIcon,
} from "./Icons";

interface DetailDrawerProps {
  card: ArchiveCardData;
  collections: Collection[];
  onClose: () => void;
  onRead: () => void;
  onNotify: (message: string, kind?: "info" | "success" | "error") => void;
  onRatingChange: (id: number, rating: number) => Promise<void>;
  onCollectionToggle: (
    archiveId: number,
    collectionId: number,
    member: boolean,
  ) => Promise<void>;
  onCollectionCover: (
    collectionId: number,
    archiveId: number | null,
  ) => Promise<void>;
  onNoteChange: (id: number, note: string | null) => void;
  onTagsChanged: () => Promise<void>;
  onDelete: (id: number) => Promise<void>;
  /**
   * 停靠模式：不铺遮罩，作为画廊 content-shell 的 flex 兄弟节点渲染，
   * 画廊网格自动让位重排（「外拓」）。
   */
  docked?: boolean;
  /** 当前停靠形态，供标题栏的切换按钮显示。 */
  dock?: "dock" | "float";
  /** 切换停靠/浮层（由 LibraryView 持久化）。不传则不显示切换按钮。 */
  onDockChange?: (dock: "dock" | "float") => void;
  /** 在阅读器里打开：隐藏「开始阅读」按钮，Esc 归阅读器统一分发。 */
  embeddedInReader?: boolean;
}

const RATING_PRESETS = [0, 20, 40, 60, 80, 100];
const RATING_SAVE_DELAY = 450;

type RatingSaveState = "idle" | "pending" | "saving" | "saved" | "error";

const SAVE_LABELS: Record<RatingSaveState, string> = {
  idle: "",
  pending: "待保存",
  saving: "保存中…",
  saved: "已保存",
  error: "保存失败",
};

function clampRating(value: number): number {
  return Math.min(100, Math.max(0, Math.round(value)));
}

const TAG_CATEGORY_ORDER = [
  "语言",
  "原作",
  "角色",
  "作者",
  "女性",
  "男性",
  "混合",
  "其他",
];

interface TagGroup {
  key: string;
  label: string;
  tags: Tag[];
}

function groupTagsByCategory(items: Tag[]): TagGroup[] {
  const groups = new Map<string, Tag[]>();
  for (const tag of items) {
    const key = tag.category?.trim() ?? "";
    const list = groups.get(key);
    if (list) list.push(tag);
    else groups.set(key, [tag]);
  }
  const rank = (key: string) => {
    if (!key) return TAG_CATEGORY_ORDER.length + 1;
    const index = TAG_CATEGORY_ORDER.indexOf(key);
    return index === -1 ? TAG_CATEGORY_ORDER.length : index;
  };
  return [...groups.entries()]
    .sort(([a], [b]) => rank(a) - rank(b) || a.localeCompare(b, "zh-Hans-CN"))
    .map(([key, groupTags]) => ({
      key,
      label: key || "未分类",
      tags: groupTags,
    }));
}

export default function DetailDrawer({
  card,
  collections,
  onClose,
  onRead,
  onNotify,
  onRatingChange,
  onCollectionToggle,
  onCollectionCover,
  onNoteChange,
  onTagsChanged,
  onDelete,
  docked = false,
  dock = "dock",
  onDockChange,
  embeddedInReader = false,
}: DetailDrawerProps) {
  const [deleteConfirm, setDeleteConfirm] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [ratingDraft, setRatingDraft] = useState(card.rating);
  const [saveState, setSaveState] = useState<RatingSaveState>("idle");
  const [tags, setTags] = useState<Tag[]>([]);
  const [knownTags, setKnownTags] = useState<Tag[]>([]);
  const [tagInput, setTagInput] = useState("");
  const [tagBusy, setTagBusy] = useState(false);
  const [tagFocused, setTagFocused] = useState(false);
  const [tagPickerOpen, setTagPickerOpen] = useState(false);
  const [tagPickerQuery, setTagPickerQuery] = useState("");
  const [bulkTagOpen, setBulkTagOpen] = useState(false);
  const [bulkTagText, setBulkTagText] = useState("");
  const [memberIds, setMemberIds] = useState<number[]>([]);
  const [collectionBusyId, setCollectionBusyId] = useState<number | null>(null);
  const [coverBusyId, setCoverBusyId] = useState<number | null>(null);
  const [newCollectionName, setNewCollectionName] = useState("");
  const [creatingCollection, setCreatingCollection] = useState(false);
  const [noteDraft, setNoteDraft] = useState(card.note ?? "");
  const [noteBusy, setNoteBusy] = useState(false);
  const [noteState, setNoteState] = useState<"idle" | "saved" | "error">(
    "idle",
  );
  const [queues, setQueues] = useState<Queue[]>([]);
  const [queueMemberIds, setQueueMemberIds] = useState<number[]>([]);
  const [queueBusyId, setQueueBusyId] = useState<number | null>(null);

  const ratingRef = useRef(card.rating);
  const lastRequestedRef = useRef(card.rating);
  const saveTimerRef = useRef<number | null>(null);
  const statusTimerRef = useRef<number | null>(null);
  const saveSeqRef = useRef(0);
  const onRatingChangeRef = useRef(onRatingChange);
  onRatingChangeRef.current = onRatingChange;

  useEffect(() => {
    ratingRef.current = card.rating;
    lastRequestedRef.current = card.rating;
    setRatingDraft(card.rating);
    setSaveState("idle");
    setNoteDraft(card.note ?? "");
    setNoteState("idle");
  }, [card.id, card.note]);

  useEffect(() => {
    let cancelled = false;
    setTags([]);
    setKnownTags([]);
    setTagInput("");
    setTagPickerOpen(false);
    setTagPickerQuery("");
    setBulkTagOpen(false);
    setBulkTagText("");
    setMemberIds([]);

    Promise.all([
      api.listArchiveTags(card.id),
      api.listTags(),
      api.listArchiveCollections(card.id),
    ])
      .then(([archiveTags, allTags, collectionIds]) => {
        if (cancelled) return;
        setTags(archiveTags);
        setKnownTags(allTags);
        setMemberIds(collectionIds);
      })
      .catch((error) => {
        if (!cancelled) {
          onNotify(`读取标签或收藏夹失败：${String(error)}`, "error");
        }
      });

    return () => {
      cancelled = true;
    };
  }, [card.id, onNotify]);

  // 待看目录：目录最多 3 个，逐个查成员关系判断本子是否已在里面。
  // 读取失败静默降级（不出区块），不影响抽屉其他功能。
  useEffect(() => {
    let cancelled = false;
    setQueues([]);
    setQueueMemberIds([]);
    (async () => {
      try {
        const list = await api.listQueues();
        const memberships = await Promise.all(
          list.map((queue) => api.listQueueArchives(queue.id)),
        );
        if (cancelled) return;
        setQueues(list);
        setQueueMemberIds(
          list
            .filter((_, index) => memberships[index].includes(card.id))
            .map((queue) => queue.id),
        );
      } catch {
        if (!cancelled) {
          setQueues([]);
          setQueueMemberIds([]);
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [card.id]);

  const tagSuggestions = useMemo(() => {
    const query = tagInput.trim().replace(/^#/, "").toLowerCase();
    const attached = new Set(tags.map((tag) => tag.id));
    return knownTags
      .filter((tag) => !attached.has(tag.id))
      .filter(
        (tag) =>
          !query ||
          tag.name.toLowerCase().includes(query) ||
          (tag.translatedName?.toLowerCase().includes(query) ?? false),
      )
      .slice(0, 8);
  }, [knownTags, tags, tagInput]);

  const tagPickerOptions = useMemo(() => {
    const query = tagPickerQuery.trim().toLowerCase();
    return knownTags
      .filter(
        (tag) =>
          !query ||
          tag.name.toLowerCase().includes(query) ||
          (tag.translatedName?.toLowerCase().includes(query) ?? false),
      )
      .slice(0, 80);
  }, [knownTags, tagPickerQuery]);

  const tagGroups = useMemo(() => groupTagsByCategory(tags), [tags]);
  const tagPickerGroups = useMemo(
    () => groupTagsByCategory(tagPickerOptions),
    [tagPickerOptions],
  );

  const bulkParsed = useMemo(() => parseTagInput(bulkTagText), [bulkTagText]);
  const bulkNewNames = useMemo(() => {
    const attached = new Set(tags.map((tag) => tag.name.toLowerCase()));
    return bulkParsed.tags.filter(
      (tag) => !attached.has(tag.name.toLowerCase()),
    );
  }, [bulkParsed, tags]);
  const bulkCategorySummary = useMemo(
    () => summarizeByCategory(bulkParsed.tags),
    [bulkParsed],
  );

  // Esc、遮罩、关闭按钮统一走带守卫的关闭入口，避免备注草稿被静默丢弃。
  // 用 ref 持有最新处理函数，保证 keydown 监听只注册一次且总能读到最新草稿状态。
  const requestCloseRef = useRef<() => void>(() => {});

  useEffect(() => {
    const handleKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented) return;
      requestCloseRef.current();
    };
    window.addEventListener("keydown", handleKey);
    return () => window.removeEventListener("keydown", handleKey);
  }, []);

  useEffect(() => {
    return () => {
      if (statusTimerRef.current !== null) {
        window.clearTimeout(statusTimerRef.current);
      }
      if (saveTimerRef.current !== null) {
        window.clearTimeout(saveTimerRef.current);
        saveTimerRef.current = null;
        void onRatingChangeRef
          .current(card.id, ratingRef.current)
          .catch(() => {});
      }
    };
  }, [card.id]);

  const commitRating = useCallback(
    async (value: number) => {
      if (saveTimerRef.current !== null) {
        window.clearTimeout(saveTimerRef.current);
        saveTimerRef.current = null;
      }
      if (statusTimerRef.current !== null) {
        window.clearTimeout(statusTimerRef.current);
        statusTimerRef.current = null;
      }

      const next = clampRating(value);
      ratingRef.current = next;
      setRatingDraft(next);

      if (next === lastRequestedRef.current) {
        setSaveState("idle");
        return;
      }

      lastRequestedRef.current = next;
      const seq = ++saveSeqRef.current;
      setSaveState("saving");

      try {
        await onRatingChangeRef.current(card.id, next);
        if (seq === saveSeqRef.current) {
          setSaveState("saved");
          if (statusTimerRef.current !== null) {
            window.clearTimeout(statusTimerRef.current);
          }
          statusTimerRef.current = window.setTimeout(
            () => setSaveState("idle"),
            1600,
          );
        }
      } catch (error) {
        if (seq === saveSeqRef.current) {
          lastRequestedRef.current = -1;
          setSaveState("error");
          onNotify(`评分保存失败：${String(error)}`, "error");
        }
      }
    },
    [card.id, onNotify],
  );

  const scheduleRating = useCallback(
    (value: number) => {
      const next = clampRating(value);
      ratingRef.current = next;
      setRatingDraft(next);
      setSaveState("pending");

      if (saveTimerRef.current !== null) {
        window.clearTimeout(saveTimerRef.current);
      }
      saveTimerRef.current = window.setTimeout(() => {
        saveTimerRef.current = null;
        void commitRating(next);
      }, RATING_SAVE_DELAY);
    },
    [commitRating],
  );

  /**
   * 批量导入：一次 IPC、一个事务。
   *
   * 以前是「循环调 add_tag」——N 个标签 N 次往返，而且任意一条超长就把整个
   * 循环抛出去，前面加进去的留在库里、后面的全丢，用户只看到一句「导入失败」。
   * 现在后端按条收集失败原因，前端只负责提示。
   */
  async function importTags(
    drafts: { name: string; category: string | null }[],
  ): Promise<BulkTagReport> {
    const report = await api.addTagsBulk(card.id, drafts);
    const [refreshedTags, allTags] = await Promise.all([
      api.listArchiveTags(card.id),
      api.listTags(),
    ]);
    setTags(refreshedTags);
    setKnownTags(allTags);
    await onTagsChanged();
    return report;
  }

  async function submitTags(raw: string) {
    if (tagBusy) return;
    const { tags: drafts } = parseTagInput(raw);
    if (drafts.length === 0) return;

    setTagBusy(true);
    try {
      await importTags(drafts);
      setTagInput("");
    } catch (error) {
      onNotify(`添加标签失败：${String(error)}`, "error");
    } finally {
      setTagBusy(false);
    }
  }

  async function submitBulkTags() {
    if (tagBusy || bulkParsed.tags.length === 0) return;
    setTagBusy(true);
    try {
      const report = await importTags(bulkParsed.tags);
      const parts = [`已导入 ${report.linked} 个标签`];
      if (report.added > 0) parts.push(`新建 ${report.added}`);
      if (report.categorized > 0) parts.push(`补分类 ${report.categorized}`);

      if (report.failed.length > 0) {
        // 有失败就保留原文，方便用户改完再导一次。
        const first = report.failed[0];
        onNotify(
          `${parts.join(" · ")}；${report.failed.length} 个未导入：${first.name}（${first.reason}）`,
          "error",
        );
        return;
      }

      setBulkTagText("");
      setBulkTagOpen(false);
      onNotify(parts.join(" · "), "success");
    } catch (error) {
      onNotify(`批量导入标签失败：${String(error)}`, "error");
    } finally {
      setTagBusy(false);
    }
  }

  async function removeTag(tag: Tag) {
    if (tagBusy) return;
    setTagBusy(true);
    try {
      await api.removeTag(card.id, tag.id);
      setTags((previous) => previous.filter((item) => item.id !== tag.id));
      setKnownTags(await api.listTags());
      await onTagsChanged();
    } catch (error) {
      onNotify(`移除标签失败：${String(error)}`, "error");
    } finally {
      setTagBusy(false);
    }
  }

  /** 加入/移出待看目录（点亮 = 已加入）。 */
  async function toggleQueue(queue: Queue) {
    if (queueBusyId !== null) return;
    const member = queueMemberIds.includes(queue.id);
    setQueueBusyId(queue.id);
    try {
      if (member) {
        await api.removeFromQueue(queue.id, card.id);
        setQueueMemberIds((previous) =>
          previous.filter((id) => id !== queue.id),
        );
        onNotify(`已移出「${queue.name}」`, "success");
      } else {
        await api.addToQueue(queue.id, card.id);
        setQueueMemberIds((previous) =>
          previous.includes(queue.id) ? previous : [...previous, queue.id],
        );
        onNotify(`已加入「${queue.name}」`, "success");
      }
    } catch (error) {
      onNotify(`更新待看目录失败：${String(error)}`, "error");
    } finally {
      setQueueBusyId(null);
    }
  }

  async function toggleCollection(collection: Collection) {
    if (collectionBusyId !== null) return;
    const member = !memberIds.includes(collection.id);
    setCollectionBusyId(collection.id);
    try {
      await onCollectionToggle(card.id, collection.id, member);
      setMemberIds((previous) =>
        member
          ? [...previous, collection.id]
          : previous.filter((id) => id !== collection.id),
      );
    } catch (error) {
      onNotify(`更新收藏夹失败：${String(error)}`, "error");
    } finally {
      setCollectionBusyId(null);
    }
  }

  async function createAndCollect() {
    const name = newCollectionName.trim();
    if (!name || creatingCollection) return;
    setCreatingCollection(true);
    try {
      const created = await api.createCollection(name);
      await onCollectionToggle(card.id, created.id, true);
      setMemberIds((previous) =>
        previous.includes(created.id) ? previous : [...previous, created.id],
      );
      setNewCollectionName("");
      onNotify(`已新建「${created.name}」并收藏`, "success");
    } catch (error) {
      onNotify(`新建收藏夹失败：${String(error)}`, "error");
    } finally {
      setCreatingCollection(false);
    }
  }

  async function toggleCover(collection: Collection) {
    if (coverBusyId !== null) return;
    const clear = collection.coverArchiveId === card.id;
    setCoverBusyId(collection.id);
    try {
      await onCollectionCover(collection.id, clear ? null : card.id);
      onNotify(
        clear
          ? `已取消「${collection.name}」的封面`
          : `已把当前画集设为「${collection.name}」的封面`,
        "success",
      );
    } catch (error) {
      onNotify(`设置封面失败：${String(error)}`, "error");
    } finally {
      setCoverBusyId(null);
    }
  }

  async function saveNote() {
    if (noteBusy) return;
    const next = noteDraft.trim();
    setNoteBusy(true);
    try {
      const saved = await api.setNote(card.id, next || null);
      setNoteDraft(saved ?? "");
      onNoteChange(card.id, saved);
      setNoteState("saved");
      onNotify("备注已保存", "success");
    } catch (error) {
      setNoteState("error");
      onNotify(`备注保存失败：${String(error)}`, "error");
    } finally {
      setNoteBusy(false);
    }
  }

  async function reveal() {
    if (!card.filePath) {
      onNotify("没有可打开的文件路径", "error");
      return;
    }
    try {
      await revealItemInDir(card.filePath);
    } catch (error) {
      onNotify(String(error), "error");
    }
  }

  const noteDirty = noteDraft.trim() !== (card.note ?? "").trim();
  const closingRef = useRef(false);

  async function requestClose() {
    if (closingRef.current || noteBusy) return;
    if (!noteDirty) {
      onClose();
      return;
    }
    // 草稿未保存：先自动落盘再关闭，失败则保留抽屉与草稿并提示。
    // 选择自动保存而非「保存/放弃/取消」对话框，是为了零新增 UI 且绝不丢输入；
    // 后续统一模态层落地后可在此替换为 useCloseGuard 三选一。
    closingRef.current = true;
    setNoteBusy(true);
    try {
      const saved = await api.setNote(card.id, noteDraft.trim() || null);
      onNoteChange(card.id, saved);
      onNotify("备注已保存", "success");
      onClose();
    } catch (error) {
      setNoteState("error");
      onNotify(`备注未保存，已保留草稿：${String(error)}`, "error");
    } finally {
      setNoteBusy(false);
      closingRef.current = false;
    }
  }
  requestCloseRef.current = () => void requestClose();

  const hasProgress = card.lastPage > 0 && card.pageCount > 0;
  const finished = hasProgress && card.lastPage >= card.pageCount;
  const readLabel = finished
    ? "重新阅读"
    : hasProgress
      ? "继续阅读"
      : "开始阅读";

  return (
    <div
      className={`drawer-backdrop${docked ? " is-docked" : ""}${
        embeddedInReader ? " is-in-reader" : ""
      }`}
      onClick={docked ? undefined : () => void requestClose()}
    >
      <aside
        className={`detail-drawer${docked ? " is-docked" : ""}`}
        role={docked ? "complementary" : "dialog"}
        aria-modal={docked ? undefined : true}
        aria-label={card.title}
        onClick={(event) => event.stopPropagation()}
      >
        <div className="drawer-header">
          <div>
            <p className="eyebrow">ARCHIVE DETAIL</p>
            <h2>{card.title}</h2>
          </div>
          <div className="drawer-header-tools">
            {onDockChange && (
              <button
                className="icon-button"
                onClick={() => onDockChange(dock === "dock" ? "float" : "dock")}
                aria-label={dock === "dock" ? "切换为浮层" : "切换为停靠"}
                title={
                  dock === "dock"
                    ? "切换为浮层（悬浮在画廊上方）"
                    : "切换为停靠（画廊自动让位）"
                }
              >
                <SwapIcon />
              </button>
            )}
            <button
              className="icon-button"
              onClick={() => void requestClose()}
              aria-label="关闭详情"
            >
              <CloseIcon />
            </button>
          </div>
        </div>

        <div className="detail-cover">
          {card.coverPath ? (
            <img src={assetUrl(card.coverPath)} alt="" draggable={false} />
          ) : (
            <div className="cover-placeholder">
              <LogoIcon width={32} height={32} />
            </div>
          )}
        </div>

        {card.missing && (
          <div className="detail-alert is-error">
            <AlertIcon />
            <span>这个文件已不在库目录中，索引记录仍被保留。</span>
          </div>
        )}

        {card.error && (
          <div className="detail-alert">
            <AlertIcon />
            <span>{card.error}</span>
          </div>
        )}

        <div className="rating-panel">
          <div className="rating-panel-head">
            <span>
              <StarIcon width={14} height={14} />
              评分
            </span>
            <strong>
              {ratingDraft}
              <small> / 100</small>
            </strong>
          </div>
          <input
            className="rating-slider"
            type="range"
            min={0}
            max={100}
            step={1}
            value={ratingDraft}
            aria-label="评分"
            onChange={(event) => scheduleRating(Number(event.target.value))}
          />
          <div className="rating-row">
            <label className="rating-number">
              <input
                type="number"
                min={0}
                max={100}
                step={1}
                value={ratingDraft}
                aria-label="评分数值"
                onFocus={(event) => event.currentTarget.select()}
                onChange={(event) => {
                  if (event.target.value === "") return;
                  const parsed = Number(event.target.value);
                  if (Number.isNaN(parsed)) return;
                  scheduleRating(parsed);
                }}
                onBlur={() => void commitRating(ratingRef.current)}
                onKeyDown={(event) => {
                  if (event.key === "Enter") event.currentTarget.blur();
                }}
              />
              <span>/ 100</span>
            </label>
            <span className={`rating-status is-${saveState}`} role="status">
              {SAVE_LABELS[saveState] || "调整后自动保存"}
            </span>
          </div>
          <div className="rating-presets">
            {RATING_PRESETS.map((preset) => (
              <button
                key={preset}
                type="button"
                className={`rating-preset${
                  ratingDraft === preset ? " is-active" : ""
                }`}
                title={preset === 60 ? "默认评分" : undefined}
                onClick={() => void commitRating(preset)}
              >
                {preset}
              </button>
            ))}
          </div>
        </div>

        <section className="meta-panel">
          <div className="meta-panel-head">
            <span>
              <BookIcon width={14} height={14} />
              备注
            </span>
            <span className="meta-hint">
              {noteState === "saved"
                ? "已保存"
                : noteState === "error"
                  ? "保存失败"
                  : `${noteDraft.length} / 2000`}
            </span>
          </div>
          <textarea
            className="note-input"
            value={noteDraft}
            maxLength={2000}
            rows={4}
            placeholder="写下简介、作者、系列或阅读提示；备注会参与全库搜索。"
            disabled={noteBusy}
            onChange={(event) => {
              setNoteDraft(event.target.value);
              setNoteState("idle");
            }}
            onKeyDown={(event) => {
              if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) {
                event.preventDefault();
                void saveNote();
              }
            }}
          />
          <div className="note-actions">
            <span className="meta-hint">Ctrl + Enter 保存 · 参与标题/文件名搜索</span>
            <button
              type="button"
              className={`btn btn-compact ${noteDirty ? "btn-primary" : "btn-ghost"}`}
              disabled={noteBusy || !noteDirty}
              onClick={() => void saveNote()}
            >
              <CheckIcon width={13} height={13} />
              {noteBusy ? "保存中…" : "保存备注"}
            </button>
          </div>
        </section>

        <section className="meta-panel">
          <div className="meta-panel-head">
            <span>
              <TagIcon width={14} height={14} />
              标签
            </span>
            <span className="meta-hint">
              {tags.length > 0 ? `${tags.length} 个` : "尚未添加"}
            </span>
          </div>

          {tags.length > 0 && (
            <div className="tag-groups">
              {tagGroups.map((group) => (
                <div
                  className="tag-group"
                  key={group.key || "__uncategorized"}
                >
                  <div className="tag-group-head">
                    <span>{group.label}</span>
                    <small>{group.tags.length}</small>
                  </div>
                  <div className="tag-list">
                    {group.tags.map((tag) => (
                      <span
                        className="tag-chip"
                        key={tag.id}
                        title={
                          tag.translatedName && tag.translatedName !== tag.name
                            ? `${tag.translatedName}（${tag.name}）`
                            : tag.name
                        }
                      >
                        {tag.translatedName || tag.name}
                        {tag.translatedName &&
                          tag.translatedName !== tag.name && (
                            <small className="tag-original">{tag.name}</small>
                          )}
                        <button
                          type="button"
                          onClick={() => void removeTag(tag)}
                          disabled={tagBusy}
                          aria-label={`移除标签 ${tag.name}`}
                          title={`移除标签 ${tag.name}`}
                        >
                          <CloseIcon width={11} height={11} />
                        </button>
                      </span>
                    ))}
                  </div>
                </div>
              ))}
            </div>
          )}

          <div className="tag-editor">
            <input
              className="tag-input"
              value={tagInput}
              placeholder="输入标签后回车；支持逗号 / 分号批量"
              disabled={tagBusy}
              onChange={(event) => setTagInput(event.target.value)}
              onFocus={() => setTagFocused(true)}
              onBlur={() => setTagFocused(false)}
              onKeyDown={(event) => {
                if (event.key === "Escape" && tagFocused) {
                  event.stopPropagation();
                  setTagFocused(false);
                  event.currentTarget.blur();
                  return;
                }
                if (event.key === "Enter" && !event.nativeEvent.isComposing) {
                  event.preventDefault();
                  void submitTags(tagInput);
                }
              }}
            />
            {tagFocused && tagSuggestions.length > 0 && (
              <div className="tag-suggestions">
                {tagSuggestions.map((tag) => (
                  <button
                    key={tag.id}
                    type="button"
                    onMouseDown={(event) => event.preventDefault()}
                    onClick={() => void submitTags(tag.name)}
                  >
                    <span>
                      {tag.translatedName || tag.name}
                      {tag.translatedName && tag.translatedName !== tag.name && (
                        <small className="tag-original">{tag.name}</small>
                      )}
                    </span>
                    <small>{tag.count}</small>
                  </button>
                ))}
              </div>
            )}
          </div>

          <button
            type="button"
            className="tag-picker-toggle"
            onClick={() => setBulkTagOpen((current) => !current)}
          >
            <span>{bulkTagOpen ? "收起批量导入" : "批量导入标签"}</span>
            <small>一次添加多个，已存在的自动跳过</small>
          </button>

          {bulkTagOpen && (
            <div className="tag-bulk-panel">
              <textarea
                autoFocus
                rows={5}
                value={bulkTagText}
                disabled={tagBusy}
                placeholder={
                  "直接粘贴标准 tag 格式，可带命名空间：\nfemale:big breasts, sole female\ncharacter:aoba utsumihikari, tachibanasensei\nartist:mountainhan\n\n也支持纯标签，每行一个或用逗号、分号、顿号分隔：\n全彩, 纯爱；萝莉、黑白"
                }
                onChange={(event) => setBulkTagText(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Escape") {
                    event.stopPropagation();
                    setBulkTagOpen(false);
                    event.currentTarget.blur();
                    return;
                  }
                  if (
                    event.key === "Enter" &&
                    (event.ctrlKey || event.metaKey)
                  ) {
                    event.preventDefault();
                    void submitBulkTags();
                  }
                }}
              />
              {bulkCategorySummary.length > 0 && (
                <div className="tag-bulk-groups">
                  {bulkCategorySummary.map(([label, count]) => (
                    <span key={label} className="tag-bulk-group">
                      {label}
                      <b>{count}</b>
                    </span>
                  ))}
                </div>
              )}
              <div className="tag-bulk-foot">
                <span>
                  {bulkParsed.tags.length === 0
                    ? "尚未识别到标签"
                    : `识别 ${bulkParsed.tags.length} 个 · 新增 ${bulkNewNames.length} 个 · 已存在 ${
                        bulkParsed.tags.length - bulkNewNames.length
                      } 个${
                        bulkParsed.rejected.length > 0
                          ? ` · 跳过 ${bulkParsed.rejected.length} 个`
                          : ""
                      }`}
                </span>
                <button
                  type="button"
                  className="btn btn-primary btn-compact"
                  disabled={tagBusy || bulkNewNames.length === 0}
                  onClick={() => void submitBulkTags()}
                >
                  <PlusIcon width={12} height={12} />
                  {tagBusy
                    ? "导入中…"
                    : bulkNewNames.length === 0
                      ? "都已存在"
                      : `导入 ${bulkNewNames.length} 个`}
                </button>
              </div>
              <p className="tag-bulk-hint">
                命名空间（female / character / artist / parody / language /
                other …）会自动映射到大类，新标签直接落进对应分组；
                <code>分类/子类: 标签</code> 可以指定路径。
              </p>
              {bulkParsed.rejected.length > 0 && (
                <p className="tag-bulk-warn">
                  <AlertIcon width={12} height={12} />
                  {bulkParsed.rejected.length} 个标签无法导入：
                  {bulkParsed.rejected
                    .slice(0, 3)
                    .map((item) => `${item.name}（${item.reason}）`)
                    .join("、")}
                  {bulkParsed.rejected.length > 3 ? " 等" : ""}
                </p>
              )}
            </div>
          )}

          <button
            type="button"
            className="tag-picker-toggle"
            onClick={() => setTagPickerOpen((current) => !current)}
          >
            <span>
              {tagPickerOpen ? "收起已有标签" : "从已有标签选择"}
            </span>
            <small>{knownTags.length} 个可用</small>
          </button>

          {tagPickerOpen && (
            <div className="tag-picker">
              <div className="tag-picker-search">
                <SearchIcon width={14} height={14} />
                <input
                  autoFocus
                  value={tagPickerQuery}
                  placeholder="搜索标签或中文译名"
                  onChange={(event) => setTagPickerQuery(event.target.value)}
                />
              </div>
              <div className="tag-picker-list">
                {tagPickerOptions.length === 0 ? (
                  <p className="tag-picker-empty">
                    {tagPickerQuery ? "没有匹配的标签" : "还没有历史标签"}
                  </p>
                ) : (
                  tagPickerGroups.map((group) => (
                    <div
                      className="tag-picker-group"
                      key={group.key || "__uncategorized"}
                    >
                      <div className="tag-group-head tag-picker-group-head">
                        <span>{group.label}</span>
                        <small>{group.tags.length}</small>
                      </div>
                      {group.tags.map((tag) => {
                        const attached = tags.some(
                          (item) => item.id === tag.id,
                        );
                        return (
                          <button
                            key={tag.id}
                            type="button"
                            className={`tag-picker-option${
                              attached ? " is-attached" : ""
                            }`}
                            disabled={tagBusy}
                            onClick={() =>
                              attached
                                ? void removeTag(tag)
                                : void submitTags(tag.name)
                            }
                            title={
                              attached
                                ? `点击从当前画集移除「${tag.name}」`
                                : `点击添加到当前画集（${tag.count} 个画集使用）`
                            }
                          >
                            <span className="tag-picker-check">
                              {attached && <CheckIcon width={11} height={11} />}
                            </span>
                            <span className="tag-picker-name">
                              {tag.translatedName || tag.name}
                              {tag.translatedName &&
                                tag.translatedName !== tag.name && (
                                  <small>{tag.name}</small>
                                )}
                            </span>
                            <span className="tag-picker-count">
                              {tag.count}
                            </span>
                          </button>
                        );
                      })}
                    </div>
                  ))
                )}
              </div>
            </div>
          )}
        </section>

        <section className="meta-panel">
          <div className="meta-panel-head">
            <span>
              <HeartIcon width={14} height={14} />
              收藏夹
            </span>
            <span className="meta-hint">
              {memberIds.length > 0
                ? `已加入 ${memberIds.length} 个`
                : "未收藏"}
            </span>
          </div>

          <div className="collection-toggles">
            {collections.map((collection) => {
              const active = memberIds.includes(collection.id);
              const isCover = collection.coverArchiveId === card.id;
              return (
                <div className="collection-row" key={collection.id}>
                  <button
                    type="button"
                    className={`collection-toggle${active ? " is-active" : ""}`}
                    onClick={() => void toggleCollection(collection)}
                    disabled={collectionBusyId !== null}
                  >
                    <span className="collection-check">
                      {active && <CheckIcon width={12} height={12} />}
                    </span>
                    <span className="collection-name">{collection.name}</span>
                    <span className="collection-count">{collection.count}</span>
                  </button>
                  {active && (
                    <button
                      type="button"
                      className={`collection-cover-btn${
                        isCover ? " is-cover" : ""
                      }`}
                      onClick={() => void toggleCover(collection)}
                      disabled={coverBusyId !== null}
                      title={
                        isCover
                          ? "取消当前封面"
                          : `把「${card.title}」设为「${collection.name}」的封面`
                      }
                      aria-label={
                        isCover
                          ? `取消「${collection.name}」的封面`
                          : `设为「${collection.name}」的封面`
                      }
                    >
                      <StarIcon width={12} height={12} />
                    </button>
                  )}
                </div>
              );
            })}
          </div>

          <form
            className="collection-inline-create"
            onSubmit={(event) => {
              event.preventDefault();
              void createAndCollect();
            }}
          >
            <input
              value={newCollectionName}
              maxLength={24}
              placeholder="新建收藏夹并收藏…"
              disabled={creatingCollection}
              onChange={(event) => setNewCollectionName(event.target.value)}
            />
            <button
              type="submit"
              className="btn btn-ghost btn-compact"
              disabled={creatingCollection || !newCollectionName.trim()}
            >
              <PlusIcon width={12} height={12} />
              {creatingCollection ? "创建中…" : "新建"}
            </button>
          </form>
        </section>

        {queues.length > 0 && (
          <section className="meta-panel">
            <div className="meta-panel-head">
              <span>
                <BookIcon width={14} height={14} />
                待看目录
              </span>
              <span className="meta-hint">
                {queueMemberIds.length > 0
                  ? `已加入 ${queueMemberIds.length} 个`
                  : "未加入"}
              </span>
            </div>

            <div className="collection-toggles">
              {queues.map((queue) => {
                const active = queueMemberIds.includes(queue.id);
                return (
                  <button
                    key={queue.id}
                    type="button"
                    className={`collection-toggle${active ? " is-active" : ""}`}
                    onClick={() => void toggleQueue(queue)}
                    disabled={queueBusyId !== null}
                    title={active ? "点击移出该目录" : "点击加入该目录"}
                  >
                    <span className="collection-check">
                      {active && <CheckIcon width={12} height={12} />}
                    </span>
                    <span className="collection-name">{queue.name}</span>
                    <span className="collection-count">{queue.count}</span>
                  </button>
                );
              })}
            </div>
          </section>
        )}

        <dl className="detail-list">
          <div>
            <dt>文件</dt>
            <dd title={card.fileName}>{card.fileName}</dd>
          </div>
          <div>
            <dt>格式</dt>
            <dd>{card.ext.toUpperCase()}</dd>
          </div>
          <div>
            <dt>大小</dt>
            <dd>{formatBytes(card.fileSize)}</dd>
          </div>
          <div>
            <dt>页数</dt>
            <dd>{card.pageCount > 0 ? `${card.pageCount} 页` : "—"}</dd>
          </div>
          <div>
            <dt>阅读进度</dt>
            <dd>
              {hasProgress
                ? `${card.lastPage} / ${card.pageCount} 页${
                    card.lastReadAt
                      ? ` · ${formatDateTime(card.lastReadAt)}`
                      : ""
                  }`
                : "未开始"}
            </dd>
          </div>
          <div>
            <dt>添加时间</dt>
            <dd>{formatDateTime(card.addedAt)}</dd>
          </div>
          <div>
            <dt>修改时间</dt>
            <dd>{formatDateTime(card.fileMtime)}</dd>
          </div>
          <div className="detail-path-row">
            <dt>路径</dt>
            <dd>
              <code>{card.filePath || "—"}</code>
            </dd>
          </div>
        </dl>

        <div className="drawer-actions">
          {!embeddedInReader && (
            <button
              className="btn btn-primary"
              onClick={onRead}
              title={
                hasProgress && !finished
                  ? `从第 ${card.lastPage} 页继续`
                  : undefined
              }
            >
              <BookIcon />
              {readLabel}
            </button>
          )}
          <button className="btn btn-ghost" onClick={reveal}>
            <RevealIcon />
            显示文件
          </button>
          <button
            className={`btn btn-compact${
              deleteConfirm ? " btn-danger" : " btn-quiet"
            }`}
            disabled={deleting}
            onClick={async () => {
              if (deleting) return;
              if (!deleteConfirm) {
                setDeleteConfirm(true);
                window.setTimeout(() => setDeleteConfirm(false), 3200);
                return;
              }
              setDeleting(true);
              try {
                await onDelete(card.id);
              } catch (error) {
                onNotify(`移入回收站失败：${String(error)}`, "error");
                setDeleting(false);
                setDeleteConfirm(false);
              }
            }}
            title={
              deleteConfirm
                ? "再次点击确认移入回收站"
                : "移入回收站，不会删除原始文件"
            }
          >
            <TrashIcon width={13} height={13} />
            {deleting ? "处理中…" : deleteConfirm ? "确认删除" : "移入回收站"}
          </button>
          <button className="btn btn-ghost" onClick={() => void requestClose()}>
            关闭
          </button>
        </div>
      </aside>
    </div>
  );
}
