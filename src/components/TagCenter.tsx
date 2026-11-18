import {
  useCallback,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { api, assetUrl } from "../api";
import { formatDateTime } from "../format";
import type {
  ArchiveCardData,
  Tag,
  TagCategory,
  TagGraph as TagGraphData,
} from "../types";
import {
  BookIcon,
  CheckIcon,
  ChevronRightIcon,
  CloseIcon,
  FolderIcon,
  GraphIcon,
  PlusIcon,
  SearchIcon,
  TagIcon,
  TrashIcon,
} from "./Icons";
import TagGraph from "./TagGraph";

interface TagCenterProps {
  onClose: () => void;
  onNotify: (message: string, kind?: "info" | "success" | "error") => void;
  onChanged: () => Promise<void>;
  onBrowseTag: (tagId: number) => void;
}

type TagTab = "active" | "categories" | "deleted";

export default function TagCenter({
  onClose,
  onNotify,
  onChanged,
  onBrowseTag,
}: TagCenterProps) {
  const [tags, setTags] = useState<Tag[]>([]);
  const [deletedTags, setDeletedTags] = useState<Tag[]>([]);
  const [categories, setCategories] = useState<TagCategory[]>([]);
  const [loading, setLoading] = useState(true);
  const [query, setQuery] = useState("");
  const [tab, setTab] = useState<TagTab>("active");
  const [selectedIds, setSelectedIds] = useState<number[]>([]);
  const [mergeTargetId, setMergeTargetId] = useState<number | "">("");
  const [batchBusy, setBatchBusy] = useState(false);
  const [graph, setGraph] = useState<TagGraphData | null>(null);
  const [graphLoading, setGraphLoading] = useState(false);
  const [graphMinWeight, setGraphMinWeight] = useState(1);
  const [collapsedCategories, setCollapsedCategories] = useState<Set<string>>(
    () => {
      try {
        const raw = window.localStorage.getItem("tagcenter-collapsed");
        const list: unknown = raw ? JSON.parse(raw) : [];
        return new Set(
          Array.isArray(list)
            ? list.filter((item): item is string => typeof item === "string")
            : [],
        );
      } catch {
        return new Set();
      }
    },
  );

  const toggleCategoryCollapsed = useCallback((path: string) => {
    setCollapsedCategories((previous) => {
      const next = new Set(previous);
      if (next.has(path)) {
        next.delete(path);
      } else {
        next.add(path);
      }
      try {
        window.localStorage.setItem(
          "tagcenter-collapsed",
          JSON.stringify([...next]),
        );
      } catch {
        // localStorage 不可用时只影响本次记忆，不影响功能
      }
      return next;
    });
  }, []);

  const loadTags = useCallback(async () => {
    setLoading(true);
    try {
      const [active, deleted, categoryList] = await Promise.all([
        api.listTags(),
        api.listDeletedTags(),
        api.listTagCategories(),
      ]);
      setTags(active);
      setDeletedTags(deleted);
      setCategories(categoryList);
      setSelectedIds((previous) =>
        previous.filter((id) => active.some((tag) => tag.id === id)),
      );
      setMergeTargetId((previous) =>
        previous !== "" && active.some((tag) => tag.id === previous)
          ? previous
          : "",
      );
    } catch (error) {
      onNotify(`标签加载失败：${String(error)}`, "error");
    } finally {
      setLoading(false);
    }
  }, [onNotify]);

  useEffect(() => {
    void loadTags();
  }, [loadTags]);

  useEffect(() => {
    const handleKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      if (graph) {
        setGraph(null);
      } else {
        onClose();
      }
    };
    window.addEventListener("keydown", handleKey);
    return () => window.removeEventListener("keydown", handleKey);
  }, [graph, onClose]);

  const filtered = useMemo(() => {
    const keyword = query.trim().toLowerCase();
    if (!keyword) return tags;
    return tags.filter((tag) => {
      const translated = tag.translatedName?.toLowerCase() ?? "";
      return (
        tag.name.toLowerCase().includes(keyword) ||
        tag.originalName.toLowerCase().includes(keyword) ||
        translated.includes(keyword) ||
        String(tag.number) === keyword.replace(/^#/, "")
      );
    });
  }, [query, tags]);

  const filteredDeleted = useMemo(() => {
    const keyword = query.trim().toLowerCase();
    if (!keyword) return deletedTags;
    return deletedTags.filter((tag) => {
      const translated = tag.translatedName?.toLowerCase() ?? "";
      return (
        tag.name.toLowerCase().includes(keyword) ||
        tag.originalName.toLowerCase().includes(keyword) ||
        translated.includes(keyword) ||
        String(tag.number) === keyword.replace(/^#/, "")
      );
    });
  }, [deletedTags, query]);

  const categoryPaths = useMemo(
    () => new Set(categories.map((category) => category.path)),
    [categories],
  );

  const directTagsByCategory = useMemo(() => {
    const map = new Map<string, Tag[]>();
    for (const tag of filtered) {
      if (!tag.category || !categoryPaths.has(tag.category)) continue;
      const bucket = map.get(tag.category) ?? [];
      bucket.push(tag);
      map.set(tag.category, bucket);
    }
    return map;
  }, [categoryPaths, filtered]);

  const uncategorizedTags = useMemo(
    () =>
      filtered.filter(
        (tag) => !tag.category || !categoryPaths.has(tag.category),
      ),
    [categoryPaths, filtered],
  );

  const childrenByCategory = useMemo(() => {
    const map = new Map<number | null, TagCategory[]>();
    for (const category of categories) {
      const bucket = map.get(category.parentId) ?? [];
      bucket.push(category);
      map.set(category.parentId, bucket);
    }
    for (const bucket of map.values()) {
      bucket.sort((left, right) =>
        left.name.localeCompare(right.name, "zh"),
      );
    }
    return map;
  }, [categories]);

  const searching = query.trim().length > 0;
  const visibleCategoryPaths = useMemo(() => {
    if (!searching) return categoryPaths;
    const visible = new Set<string>();
    for (const tag of filtered) {
      if (!tag.category || !categoryPaths.has(tag.category)) continue;
      const segments = tag.category.split("/");
      for (let index = 1; index <= segments.length; index += 1) {
        visible.add(segments.slice(0, index).join("/"));
      }
    }
    return visible;
  }, [categoryPaths, filtered, searching]);

  const categoryCounts = useMemo(() => {
    const counts = new Map<string, { direct: number; total: number }>();
    for (const category of categories) {
      counts.set(category.path, { direct: 0, total: 0 });
    }
    for (const tag of tags) {
      if (!tag.category || !counts.has(tag.category)) continue;
      const segments = tag.category.split("/");
      for (let index = 1; index <= segments.length; index += 1) {
        const key = segments.slice(0, index).join("/");
        const entry = counts.get(key);
        if (entry) entry.total += 1;
      }
      const direct = counts.get(tag.category);
      if (direct) direct.direct += 1;
    }
    return counts;
  }, [categories, tags]);

  const usedCount = tags.filter((tag) => tag.count > 0).length;
  const selectedTags = tags.filter((tag) => selectedIds.includes(tag.id));
  const mergeOptions = tags.filter((tag) => !selectedIds.includes(tag.id));

  async function openGraph() {
    if (graphLoading) return;
    setGraphLoading(true);
    try {
      setGraph(await api.tagGraph(graphMinWeight, 300));
    } catch (error) {
      onNotify(`生成导图失败：${String(error)}`, "error");
    } finally {
      setGraphLoading(false);
    }
  }

  const toggleSelect = useCallback((tagId: number) => {
    setSelectedIds((previous) =>
      previous.includes(tagId)
        ? previous.filter((id) => id !== tagId)
        : [...previous, tagId],
    );
  }, []);

  const handleSaved = useCallback(
    async (updated: Tag) => {
      setTags((previous) =>
        previous
          .map((tag) => (tag.id === updated.id ? updated : tag))
          .sort(
            (left, right) =>
              right.count - left.count ||
              left.name.localeCompare(right.name, "zh"),
          ),
      );
      await loadTags();
      await onChanged();
    },
    [loadTags, onChanged],
  );

  const handleDeleted = useCallback(async () => {
    await loadTags();
    await onChanged();
  }, [loadTags, onChanged]);

  async function mergeSelected() {
    if (batchBusy) return;
    const target = Number(mergeTargetId);
    const sources = selectedIds.filter((id) => id !== target);
    if (!target || sources.length === 0) {
      onNotify("请选择至少一个源标签和一个不同的目标标签", "error");
      return;
    }

    setBatchBusy(true);
    try {
      const merged = await api.mergeTags(sources, target);
      setSelectedIds([]);
      setMergeTargetId("");
      await loadTags();
      await onChanged();
      onNotify(
        `已把 ${sources.length} 个标签合并到「${merged.translatedName || merged.name}」`,
        "success",
      );
    } catch (error) {
      onNotify(`合并标签失败：${String(error)}`, "error");
    } finally {
      setBatchBusy(false);
    }
  }

  async function deleteSelected() {
    if (batchBusy || selectedIds.length === 0) return;
    setBatchBusy(true);
    try {
      for (const id of selectedIds) {
        await api.deleteTag(id);
      }
      const count = selectedIds.length;
      setSelectedIds([]);
      await loadTags();
      await onChanged();
      onNotify(`已将 ${count} 个标签移入回收站`, "success");
    } catch (error) {
      onNotify(`批量删除失败：${String(error)}`, "error");
      await loadTags();
    } finally {
      setBatchBusy(false);
    }
  }

  async function restoreTag(tag: Tag) {
    if (batchBusy) return;
    setBatchBusy(true);
    try {
      await api.restoreTag(tag.id);
      await loadTags();
      await onChanged();
      onNotify(`已恢复标签「${tag.translatedName || tag.name}」`, "success");
    } catch (error) {
      onNotify(`恢复失败：${String(error)}`, "error");
    } finally {
      setBatchBusy(false);
    }
  }

  async function purgeTag(tag: Tag) {
    if (batchBusy) return;
    setBatchBusy(true);
    try {
      await api.purgeTag(tag.id);
      await loadTags();
      await onChanged();
      onNotify(`已彻底删除「${tag.translatedName || tag.name}」`, "success");
    } catch (error) {
      onNotify(`彻底删除失败：${String(error)}`, "error");
    } finally {
      setBatchBusy(false);
    }
  }

  const renderTagRow = (tag: Tag) => (
    <TagRow
      key={tag.id}
      tag={tag}
      categories={categories}
      selected={selectedIds.includes(tag.id)}
      onToggleSelect={toggleSelect}
      onNotify={onNotify}
      onSaved={handleSaved}
      onDeleted={handleDeleted}
      onBrowse={onBrowseTag}
    />
  );

  function renderCategory(category: TagCategory): ReactNode {
    if (!visibleCategoryPaths.has(category.path)) return null;
    const directTags = directTagsByCategory.get(category.path) ?? [];
    const childSections = (childrenByCategory.get(category.id) ?? [])
      .map((child) => renderCategory(child))
      .filter(Boolean);
    if (searching && directTags.length === 0 && childSections.length === 0) {
      return null;
    }

    // 搜索时忽略折叠状态，保证命中结果全部可见
    const collapsed = !searching && collapsedCategories.has(category.path);
    const totalCount =
      categoryCounts.get(category.path)?.total ?? directTags.length;

    return (
      <section
        key={category.path}
        className={`tag-category${collapsed ? " is-collapsed" : ""}`}
        style={{ marginLeft: `${Math.min(category.depth, 5) * 14}px` }}
      >
        <header
          className="tag-category-head"
          role="button"
          tabIndex={0}
          aria-expanded={!collapsed}
          onClick={() => toggleCategoryCollapsed(category.path)}
          onKeyDown={(event) => {
            if (event.key === "Enter" || event.key === " ") {
              event.preventDefault();
              toggleCategoryCollapsed(category.path);
            }
          }}
        >
          <ChevronRightIcon width={12} height={12} />
          <FolderIcon width={14} height={14} />
          <strong>{category.name}</strong>
          {category.path !== category.name && (
            <span className="tag-category-path">{category.path}</span>
          )}
          <span className="tag-category-count">
            {collapsed ? totalCount : directTags.length}
          </span>
        </header>
        {!collapsed && directTags.length > 0 && (
          <div className="tag-category-rows">
            {directTags.map(renderTagRow)}
          </div>
        )}
        {!collapsed && childSections}
      </section>
    );
  }

  function renderCollapsibleGroup(
    path: string,
    icon: ReactNode,
    title: string,
    subtitle: string,
    rows: Tag[],
  ): ReactNode {
    const collapsed = !searching && collapsedCategories.has(path);
    return (
      <section
        key={path}
        className={`tag-category is-uncategorized${collapsed ? " is-collapsed" : ""}`}
      >
        <header
          className="tag-category-head"
          role="button"
          tabIndex={0}
          aria-expanded={!collapsed}
          onClick={() => toggleCategoryCollapsed(path)}
          onKeyDown={(event) => {
            if (event.key === "Enter" || event.key === " ") {
              event.preventDefault();
              toggleCategoryCollapsed(path);
            }
          }}
        >
          <ChevronRightIcon width={12} height={12} />
          {icon}
          <strong>{title}</strong>
          <span className="tag-category-path">{subtitle}</span>
          <span className="tag-category-count">{rows.length}</span>
        </header>
        {!collapsed && (
          <div className="tag-category-rows">{rows.map(renderTagRow)}</div>
        )}
      </section>
    );
  }

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="tag-center"
        role="dialog"
        aria-modal="true"
        aria-label="标签中心"
        onClick={(event) => event.stopPropagation()}
      >
        <header className="tag-center-head">
          <div className="tag-center-title">
            <span className="tag-center-mark">
              <TagIcon width={19} height={19} />
            </span>
            <div>
              <p className="eyebrow">TAG CENTER</p>
              <h2>标签中心</h2>
              <p>
                共 {tags.length} 个标签 · {usedCount} 个已使用 ·{" "}
                {tags.length - usedCount} 个待整理 · {deletedTags.length} 个在回收站
              </p>
            </div>
          </div>
          <div className="tag-center-actions">
            <label className="mini-select" title="导图至少连线几次才显示">
              <span>连线 ≥</span>
              <select
                value={graphMinWeight}
                onChange={(event) =>
                  setGraphMinWeight(Number(event.target.value))
                }
              >
                <option value={1}>1</option>
                <option value={2}>2</option>
                <option value={3}>3</option>
                <option value={5}>5</option>
              </select>
            </label>
            <button
              type="button"
              className="btn btn-ghost btn-compact"
              onClick={() => void openGraph()}
              disabled={graphLoading || tags.length === 0}
            >
              <GraphIcon width={15} height={15} />
              {graphLoading ? "生成中…" : "一键导图"}
            </button>
            <button
              type="button"
              className="icon-button"
              onClick={onClose}
              aria-label="关闭标签中心"
            >
              <CloseIcon />
            </button>
          </div>
        </header>

        <div className="tag-center-search">
          <SearchIcon width={16} height={16} />
          <input
            value={query}
            placeholder="搜索标签、原始名、中文译名或编号…"
            onChange={(event) => setQuery(event.target.value)}
          />
          {query && (
            <button
              type="button"
              className="icon-button"
              onClick={() => setQuery("")}
              aria-label="清空搜索"
            >
              <CloseIcon width={13} height={13} />
            </button>
          )}
        </div>

        <div className="tag-tabs">
          <button
            type="button"
            className={tab === "active" ? "is-active" : ""}
            onClick={() => setTab("active")}
          >
            在用标签
            <span>{tags.length}</span>
          </button>
          <button
            type="button"
            className={tab === "categories" ? "is-active" : ""}
            onClick={() => setTab("categories")}
          >
            分类层级
            <span>{categories.length}</span>
          </button>
          <button
            type="button"
            className={tab === "deleted" ? "is-active" : ""}
            onClick={() => setTab("deleted")}
          >
            回收站
            <span>{deletedTags.length}</span>
          </button>
        </div>

        {tab === "active" && selectedIds.length > 0 && (
          <div className="tag-batch-bar">
            <span className="tag-batch-count">已选 {selectedIds.length} 个</span>
            <label className="mini-select">
              <span>合并到</span>
              <select
                value={mergeTargetId}
                onChange={(event) =>
                  setMergeTargetId(
                    event.target.value ? Number(event.target.value) : "",
                  )
                }
                disabled={batchBusy || mergeOptions.length === 0}
              >
                <option value="">选择目标…</option>
                {mergeOptions.map((tag) => (
                  <option key={tag.id} value={tag.id}>
                    #{tag.number} {tag.translatedName || tag.name}
                  </option>
                ))}
              </select>
            </label>
            <button
              type="button"
              className="btn btn-primary btn-compact"
              disabled={batchBusy || !mergeTargetId}
              onClick={() => void mergeSelected()}
            >
              合并
            </button>
            <button
              type="button"
              className="btn btn-danger btn-compact"
              disabled={batchBusy}
              onClick={() => void deleteSelected()}
            >
              <TrashIcon width={13} height={13} />
              移入回收站
            </button>
            <button
              type="button"
              className="btn btn-quiet btn-compact"
              disabled={batchBusy}
              onClick={() => setSelectedIds([])}
            >
              取消选择
            </button>
          </div>
        )}

        <div className="tag-center-list">
          {loading ? (
            <p className="tag-center-hint">正在读取标签…</p>
          ) : tab === "categories" ? (
            <CategoryManager
              categories={categories}
              counts={categoryCounts}
              onChanged={loadTags}
              onNotify={onNotify}
            />
          ) : tab === "active" ? (
            filtered.length === 0 ? (
              <div className="tag-center-empty">
                <TagIcon width={26} height={26} />
                <p>{query ? "没有匹配的标签" : "还没有标签"}</p>
                <small>
                  {query
                    ? "可以按编号（如 #12）、原始名或中文译名搜索。"
                    : "在画集详情里添加标签，英文标签会自动翻译为中文。"}
                </small>
              </div>
            ) : (
              <>
                {/* 未分类置顶：这是最常需要补分类的地方，不用每次滑到底 */}
                {uncategorizedTags.length > 0 &&
                  renderCollapsibleGroup(
                    "__uncategorized__",
                    <TagIcon width={14} height={14} />,
                    "未分类",
                    "尚未指定分类的标签",
                    uncategorizedTags,
                  )}
                {(childrenByCategory.get(null) ?? []).map((category) =>
                  renderCategory(category),
                )}
              </>
            )
          ) : filteredDeleted.length === 0 ? (
            <div className="tag-center-empty">
              <TrashIcon width={26} height={26} />
              <p>{query ? "回收站里没有匹配的标签" : "回收站是空的"}</p>
              <small>
                删除的标签会先进入回收站，随时可以恢复；也可以彻底删除。
              </small>
            </div>
          ) : (
            filteredDeleted.map((tag) => (
              <DeletedTagRow
                key={tag.id}
                tag={tag}
                busy={batchBusy}
                onRestore={restoreTag}
                onPurge={purgeTag}
              />
            ))
          )}
        </div>

        <footer className="tag-center-foot">
          <span>
            标签编号按导入顺序分配，原始名保持不变；点击数量可展开该标签下的画集预览。
          </span>
          <span>
            {selectedTags.length > 0
              ? `已选：${selectedTags.map((tag) => `#${tag.number}`).join(" ")}`
              : "删除只会移入回收站，不会立刻丢失关联。"}
          </span>
        </footer>
      </div>

      {graph && (
        <TagGraph
          graph={graph}
          onClose={() => setGraph(null)}
          onBrowseTag={onBrowseTag}
        />
      )}
    </div>
  );
}

interface TagRowProps {
  tag: Tag;
  categories: TagCategory[];
  selected: boolean;
  onToggleSelect: (tagId: number) => void;
  onNotify: (message: string, kind?: "info" | "success" | "error") => void;
  onSaved: (tag: Tag) => Promise<void>;
  onDeleted: () => Promise<void>;
  onBrowse: (tagId: number) => void;
}

function TagRow({
  tag,
  categories,
  selected,
  onToggleSelect,
  onNotify,
  onSaved,
  onDeleted,
  onBrowse,
}: TagRowProps) {
  const [name, setName] = useState(tag.name);
  const [translatedName, setTranslatedName] = useState(tag.translatedName ?? "");
  const [category, setCategory] = useState(tag.category ?? "");
  const [busy, setBusy] = useState(false);
  const [weightDraft, setWeightDraft] = useState(String(tag.weight));
  const [weightBusy, setWeightBusy] = useState(false);
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const [archivesOpen, setArchivesOpen] = useState(false);
  const [archives, setArchives] = useState<ArchiveCardData[] | null>(null);
  const [archivesTotal, setArchivesTotal] = useState(0);
  const [archivesLoading, setArchivesLoading] = useState(false);

  useEffect(() => {
    setArchivesOpen(false);
    setArchives(null);
    setArchivesTotal(0);
  }, [tag.id]);

  useEffect(() => {
    setName(tag.name);
    setTranslatedName(tag.translatedName ?? "");
    setCategory(tag.category ?? "");
    setWeightDraft(String(tag.weight));
  }, [tag.id, tag.name, tag.translatedName, tag.category, tag.weight]);

  /** 权重单独保存：它不参与「保存」按钮的 dirty 判定，失焦即提交。 */
  async function commitWeight() {
    const parsed = Number(weightDraft);
    if (!Number.isFinite(parsed)) {
      setWeightDraft(String(tag.weight));
      return;
    }
    const clamped = Math.min(100, Math.max(0, Math.trunc(parsed)));
    if (clamped === tag.weight) {
      setWeightDraft(String(tag.weight));
      return;
    }
    setWeightBusy(true);
    try {
      const updated = await api.setTagWeight(tag.id, clamped);
      setWeightDraft(String(updated.weight));
      await onSaved(updated);
      onNotify(
        updated.weight === 0
          ? `「${updated.name}」已设为 0 权重：不再进入待看目录`
          : `「${updated.name}」权重已设为 ${updated.weight}`,
        "success",
      );
    } catch (error) {
      setWeightDraft(String(tag.weight));
      onNotify(`保存权重失败：${String(error)}`, "error");
    } finally {
      setWeightBusy(false);
    }
  }

  useEffect(() => {
    if (!confirmingDelete) return;
    const timer = window.setTimeout(() => setConfirmingDelete(false), 3200);
    return () => window.clearTimeout(timer);
  }, [confirmingDelete]);

  const fieldsDirty =
    name.trim() !== tag.name ||
    translatedName.trim() !== (tag.translatedName ?? "");
  const categoryDirty = category !== (tag.category ?? "");
  const dirty = fieldsDirty || categoryDirty;

  async function save() {
    if (busy || !dirty) return;
    if (!name.trim()) {
      onNotify("标签名称不能为空", "error");
      return;
    }

    setBusy(true);
    try {
      let updated = tag;
      if (fieldsDirty) {
        updated = await api.updateTag(
          tag.id,
          name.trim(),
          translatedName.trim() || null,
        );
      }
      if (categoryDirty) {
        updated = await api.setTagCategory(tag.id, category.trim() || null);
      }
      await onSaved(updated);
      onNotify(`已更新标签「${updated.name}」`, "success");
    } catch (error) {
      onNotify(`保存标签失败：${String(error)}`, "error");
    } finally {
      setBusy(false);
    }
  }

  async function remove() {
    if (!confirmingDelete) {
      setConfirmingDelete(true);
      return;
    }

    setBusy(true);
    try {
      await api.deleteTag(tag.id);
      await onDeleted();
      onNotify(`已把标签「${tag.name}」移入回收站`, "success");
    } catch (error) {
      onNotify(`删除标签失败：${String(error)}`, "error");
      setBusy(false);
      setConfirmingDelete(false);
    }
  }

  async function toggleArchives() {
    if (tag.count === 0) return;
    if (archivesOpen) {
      setArchivesOpen(false);
      return;
    }
    setArchivesOpen(true);
    if (archives !== null || archivesLoading) return;

    setArchivesLoading(true);
    try {
      const page = await api.queryArchives({
        tagIds: [tag.id],
        tagMode: "and",
        pageSize: 24,
        sort: "added_at",
        direction: "desc",
      });
      setArchives(page.items);
      setArchivesTotal(page.total);
    } catch (error) {
      onNotify(`读取标签画集失败：${String(error)}`, "error");
      setArchivesOpen(false);
    } finally {
      setArchivesLoading(false);
    }
  }

  return (
    <div className={`tag-row${selected ? " is-selected" : ""}`}>
      <button
        type="button"
        className={`tag-row-select${selected ? " is-on" : ""}`}
        onClick={() => onToggleSelect(tag.id)}
        aria-label={selected ? `取消选择 ${tag.name}` : `选择 ${tag.name}`}
        title="选择后可批量合并或删除"
      >
        {selected && <CheckIcon width={11} height={11} />}
      </button>

      <span className="tag-row-number" title="按导入顺序分配的编号">
        #{tag.number}
      </span>

      <button
        type="button"
        className={`tag-row-count${tag.count === 0 ? " is-empty" : ""}${
          archivesOpen ? " is-open" : ""
        }`}
        onClick={() => void toggleArchives()}
        disabled={tag.count === 0}
        aria-expanded={archivesOpen}
        title={
          tag.count === 0 ? "尚未使用" : `查看带此标签的 ${tag.count} 个画集`
        }
      >
        {tag.count}
      </button>

      <div className="tag-row-fields">
        <label>
          <span>显示名</span>
          <input
            value={name}
            disabled={busy}
            onChange={(event) => setName(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") void save();
            }}
          />
        </label>
        <label>
          <span>中文译名</span>
          <input
            value={translatedName}
            placeholder="可选，AI 批量翻译会写入这里"
            disabled={busy}
            onChange={(event) => setTranslatedName(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") void save();
            }}
          />
        </label>
        <label>
          <span>分类</span>
          <select
            value={category}
            disabled={busy}
            onChange={(event) => setCategory(event.target.value)}
            title={category || "未分类"}
          >
            <option value="">未分类</option>
            {categories.map((item) => (
              <option key={item.path} value={item.path}>
                {`${"— ".repeat(item.depth)}${item.name}`}
              </option>
            ))}
          </select>
        </label>
        <label title="待看目录随机挑选时的权重；0 表示不想看到带这个标签的画集">
          <span>权重</span>
          <input
            type="number"
            min={0}
            max={100}
            className={tag.weight === 0 ? "is-zero-weight" : undefined}
            value={weightDraft}
            disabled={busy || weightBusy}
            onChange={(event) =>
              setWeightDraft(event.target.value.replace(/[^\d]/g, ""))
            }
            onBlur={() => void commitWeight()}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                void commitWeight();
              }
            }}
          />
        </label>
        {tag.weight === 0 && (
          <p className="tag-row-origin is-warning">
            0 权重：生成待看目录时会排除带此标签的画集
          </p>
        )}
        {tag.originalName !== tag.name && (
          <p className="tag-row-origin">
            原始名：{tag.originalName}（不随显示名修改）
            {tag.translationSource
              ? ` · 译名来源：${tag.translationSource}`
              : ""}
          </p>
        )}
      </div>

      <div className="tag-row-actions">
        <button
          type="button"
          className={`btn btn-compact ${
            dirty ? "btn-primary" : "btn-ghost"
          }`}
          disabled={busy || !dirty}
          onClick={() => void save()}
          title="保存显示名与译名"
        >
          <CheckIcon width={13} height={13} />
          保存
        </button>
        <button
          type="button"
          className="btn btn-ghost btn-compact"
          disabled={busy || tag.count === 0}
          onClick={() => onBrowse(tag.id)}
          title={
            tag.count === 0 ? "这个标签还没有关联画集" : "只看带此标签的画集"
          }
        >
          <BookIcon width={13} height={13} />
          画集
        </button>
        <button
          type="button"
          className={`btn btn-compact ${
            confirmingDelete ? "btn-danger" : "btn-quiet"
          }`}
          disabled={busy}
          onClick={() => void remove()}
          title={confirmingDelete ? "再次点击移入回收站" : "移入回收站"}
        >
          <TrashIcon width={13} height={13} />
          {confirmingDelete ? "确认" : "删除"}
        </button>
      </div>

      {archivesOpen && (
        <div className="tag-archives-panel">
          {archivesLoading ? (
            <p className="tag-archives-hint">正在读取画集…</p>
          ) : archives && archives.length > 0 ? (
            <>
              <div className="tag-archives-grid">
                {archives.map((archive) => (
                  <button
                    key={archive.id}
                    type="button"
                    className="tag-archive-card"
                    onClick={() => onBrowse(tag.id)}
                    title={`${archive.title} · ${archive.pageCount} 页 · 评分 ${archive.rating}${
                      archive.missing ? " · 文件缺失" : ""
                    }`}
                  >
                    <span className="tag-archive-cover">
                      {archive.coverPath ? (
                        <img
                          src={assetUrl(archive.coverPath)}
                          alt=""
                          loading="lazy"
                          draggable={false}
                        />
                      ) : (
                        <BookIcon width={16} height={16} />
                      )}
                    </span>
                    <span className="tag-archive-title">{archive.title}</span>
                    <span className="tag-archive-meta">
                      {archive.missing ? "文件缺失 · " : ""}
                      评分 {archive.rating} · {archive.pageCount} 页
                    </span>
                  </button>
                ))}
              </div>
              <div className="tag-archives-foot">
                <span>
                  {archivesTotal > archives.length
                    ? `显示前 ${archives.length} 本 · 共 ${archivesTotal} 本`
                    : `共 ${archives.length} 本画集`}
                </span>
                <button
                  type="button"
                  className="btn btn-ghost btn-compact"
                  onClick={() => onBrowse(tag.id)}
                >
                  <BookIcon width={13} height={13} />
                  在画廊中筛选
                </button>
              </div>
            </>
          ) : (
            <p className="tag-archives-hint">这个标签下还没有画集</p>
          )}
        </div>
      )}
    </div>
  );
}

interface DeletedTagRowProps {
  tag: Tag;
  busy: boolean;
  onRestore: (tag: Tag) => Promise<void>;
  onPurge: (tag: Tag) => Promise<void>;
}

function DeletedTagRow({ tag, busy, onRestore, onPurge }: DeletedTagRowProps) {
  const [confirmingPurge, setConfirmingPurge] = useState(false);

  useEffect(() => {
    if (!confirmingPurge) return;
    const timer = window.setTimeout(() => setConfirmingPurge(false), 3200);
    return () => window.clearTimeout(timer);
  }, [confirmingPurge]);

  const label = tag.translatedName || tag.name;
  const merged = tag.mergedInto !== null;

  return (
    <div className="tag-row is-deleted">
      <span className="tag-row-number">#{tag.number}</span>
      <div className="tag-row-summary">
        <strong>{label}</strong>
        <small>
          原始名：{tag.originalName} · 删除于{" "}
          {tag.deletedAt ? formatDateTime(tag.deletedAt) : "—"}
        </small>
        {merged ? (
          <small className="is-merged">
            已合并到标签 #{tag.mergedInto}，无法单独恢复
          </small>
        ) : (
          <small>{tag.count} 个画集仍保留此标签的关联</small>
        )}
      </div>
      <div className="tag-row-actions">
        <button
          type="button"
          className="btn btn-ghost btn-compact"
          disabled={busy || merged}
          onClick={() => void onRestore(tag)}
          title={merged ? "合并产生的源标签不能单独恢复" : "恢复到在用标签"}
        >
          恢复
        </button>
        <button
          type="button"
          className={`btn btn-compact ${
            confirmingPurge ? "btn-danger" : "btn-quiet"
          }`}
          disabled={busy}
          onClick={() => {
            if (!confirmingPurge) {
              setConfirmingPurge(true);
              return;
            }
            setConfirmingPurge(false);
            void onPurge(tag);
          }}
          title={confirmingPurge ? "再次点击彻底删除" : "彻底删除，无法恢复"}
        >
          <TrashIcon width={13} height={13} />
          {confirmingPurge ? "确认彻底删除" : "彻底删除"}
        </button>
      </div>
    </div>
  );
}

interface CategoryManagerProps {
  categories: TagCategory[];
  counts: Map<string, { direct: number; total: number }>;
  onChanged: () => Promise<void>;
  onNotify: (message: string, kind?: "info" | "success" | "error") => void;
}

function CategoryManager({
  categories,
  counts,
  onChanged,
  onNotify,
}: CategoryManagerProps) {
  const [newRoot, setNewRoot] = useState("");
  const [busy, setBusy] = useState(false);
  const roots = categories.filter((category) => category.parentId === null);

  async function createRoot() {
    const name = newRoot.trim();
    if (busy || !name) return;
    setBusy(true);
    try {
      const created = await api.createTagCategory(null, name);
      setNewRoot("");
      await onChanged();
      onNotify(`已创建分类「${created.path}」`, "success");
    } catch (error) {
      onNotify(`创建分类失败：${String(error)}`, "error");
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="tag-category-manager">
      <div className="tag-category-create">
        <input
          value={newRoot}
          placeholder="新建根分类，例如 主题、画师、场景…"
          disabled={busy}
          onChange={(event) => setNewRoot(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter") void createRoot();
          }}
        />
        <button
          type="button"
          className="btn btn-primary btn-compact"
          disabled={busy || !newRoot.trim()}
          onClick={() => void createRoot()}
        >
          <PlusIcon width={13} height={13} />
          新建根分类
        </button>
      </div>

      {categories.length === 0 ? (
        <div className="tag-center-empty">
          <FolderIcon width={26} height={26} />
          <p>还没有分类</p>
          <small>
            先建立根分类，再进入子分类；标签可以在「在用标签」里选择归属分类。
          </small>
        </div>
      ) : (
        <div className="tag-category-tree">
          {roots.map((category) => (
            <CategoryNode
              key={category.path}
              category={category}
              categories={categories}
              counts={counts}
              onChanged={onChanged}
              onNotify={onNotify}
            />
          ))}
        </div>
      )}
    </div>
  );
}

interface CategoryNodeProps {
  category: TagCategory;
  categories: TagCategory[];
  counts: Map<string, { direct: number; total: number }>;
  onChanged: () => Promise<void>;
  onNotify: (message: string, kind?: "info" | "success" | "error") => void;
}

function CategoryNode({
  category,
  categories,
  counts,
  onChanged,
  onNotify,
}: CategoryNodeProps) {
  const [action, setAction] = useState<"child" | "rename" | "move" | null>(
    null,
  );
  const [draft, setDraft] = useState("");
  const [moveTarget, setMoveTarget] = useState("");
  const [busy, setBusy] = useState(false);
  const [confirmingDelete, setConfirmingDelete] = useState(false);

  useEffect(() => {
    if (!confirmingDelete) return;
    const timer = window.setTimeout(() => setConfirmingDelete(false), 3200);
    return () => window.clearTimeout(timer);
  }, [confirmingDelete]);

  const row = counts.get(category.path) ?? { direct: 0, total: 0 };
  const children = categories
    .filter((item) => item.parentId === category.id)
    .sort((left, right) => left.name.localeCompare(right.name, "zh"));
  const parent = categories.find((item) => item.id === category.parentId);
  const moveOptions = categories
    .filter(
      (item) =>
        item.path !== category.path &&
        !item.path.startsWith(`${category.path}/`),
    )
    .sort((left, right) => left.path.localeCompare(right.path, "zh"));

  function toggleAction(next: "child" | "rename" | "move") {
    if (action === next) {
      setAction(null);
      return;
    }
    setAction(next);
    if (next === "child") setDraft("");
    if (next === "rename") setDraft(category.name);
    if (next === "move") setMoveTarget(parent?.path ?? "");
  }

  async function createChild() {
    const name = draft.trim();
    if (busy || !name) return;
    setBusy(true);
    try {
      const created = await api.createTagCategory(category.path, name);
      setAction(null);
      await onChanged();
      onNotify(`已在「${category.path}」下创建「${created.name}」`, "success");
    } catch (error) {
      onNotify(`创建子分类失败：${String(error)}`, "error");
    } finally {
      setBusy(false);
    }
  }

  async function rename() {
    const name = draft.trim();
    if (busy || !name || name === category.name) return;
    setBusy(true);
    try {
      await api.renameTagCategory(category.path, name);
      setAction(null);
      await onChanged();
      onNotify(`已将分类重命名为「${name}」`, "success");
    } catch (error) {
      onNotify(`重命名分类失败：${String(error)}`, "error");
    } finally {
      setBusy(false);
    }
  }

  async function move() {
    if (busy) return;
    setBusy(true);
    try {
      await api.moveTagCategory(category.path, moveTarget || null);
      setAction(null);
      await onChanged();
      onNotify(
        moveTarget
          ? `已将「${category.path}」移动到「${moveTarget}」`
          : `已将「${category.path}」移到根分类`,
        "success",
      );
    } catch (error) {
      onNotify(`移动分类失败：${String(error)}`, "error");
    } finally {
      setBusy(false);
    }
  }

  async function remove() {
    if (busy) return;
    if (!confirmingDelete) {
      setConfirmingDelete(true);
      return;
    }
    setBusy(true);
    try {
      await api.deleteTagCategory(category.path);
      setConfirmingDelete(false);
      await onChanged();
      onNotify(
        `已删除分类「${category.path}」，其下标签回到未分类`,
        "success",
      );
    } catch (error) {
      onNotify(`删除分类失败：${String(error)}`, "error");
      setBusy(false);
      setConfirmingDelete(false);
    }
  }

  return (
    <div
      className="tag-category-node"
      style={{ marginLeft: category.parentId ? "14px" : 0 }}
    >
      <div className="tag-category-node-row">
        <span className="tag-category-node-name">
          <FolderIcon width={14} height={14} />
          <strong>{category.name}</strong>
        </span>
        {category.path !== category.name && (
          <span className="tag-category-node-path">{category.path}</span>
        )}
        <span className="tag-category-node-count">
          {row.direct} 个标签
          {children.length > 0 ? ` · 含子分类 ${row.total}` : ""}
        </span>
        <div className="tag-category-node-actions">
          <button
            type="button"
            className="btn btn-ghost btn-compact"
            disabled={busy || category.depth >= 5}
            onClick={() => toggleAction("child")}
            title={
              category.depth >= 5
                ? "最多支持 6 级分类"
                : "在此分类下新建子分类"
            }
          >
            <PlusIcon width={12} height={12} />
            子分类
          </button>
          <button
            type="button"
            className="btn btn-ghost btn-compact"
            disabled={busy}
            onClick={() => toggleAction("rename")}
          >
            重命名
          </button>
          <button
            type="button"
            className="btn btn-ghost btn-compact"
            disabled={busy}
            onClick={() => toggleAction("move")}
          >
            移动
          </button>
          <button
            type="button"
            className={`btn btn-compact ${
              confirmingDelete ? "btn-danger" : "btn-quiet"
            }`}
            disabled={busy}
            onClick={() => void remove()}
            title={
              confirmingDelete
                ? "再次点击删除分类及其子分类"
                : "删除分类及其子分类，标签保留"
            }
          >
            <TrashIcon width={12} height={12} />
            {confirmingDelete ? "确认删除" : "删除"}
          </button>
        </div>
      </div>

      {action === "child" && (
        <div className="tag-category-inline">
          <input
            autoFocus
            value={draft}
            placeholder={`在「${category.name}」下新建子分类`}
            disabled={busy}
            onChange={(event) => setDraft(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") void createChild();
              if (event.key === "Escape") setAction(null);
            }}
          />
          <button
            type="button"
            className="btn btn-primary btn-compact"
            disabled={busy || !draft.trim()}
            onClick={() => void createChild()}
          >
            创建
          </button>
          <button
            type="button"
            className="btn btn-quiet btn-compact"
            disabled={busy}
            onClick={() => setAction(null)}
          >
            取消
          </button>
        </div>
      )}

      {action === "rename" && (
        <div className="tag-category-inline">
          <input
            autoFocus
            value={draft}
            disabled={busy}
            onChange={(event) => setDraft(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") void rename();
              if (event.key === "Escape") setAction(null);
            }}
          />
          <button
            type="button"
            className="btn btn-primary btn-compact"
            disabled={busy || !draft.trim() || draft.trim() === category.name}
            onClick={() => void rename()}
          >
            保存名称
          </button>
          <button
            type="button"
            className="btn btn-quiet btn-compact"
            disabled={busy}
            onClick={() => setAction(null)}
          >
            取消
          </button>
        </div>
      )}

      {action === "move" && (
        <div className="tag-category-inline">
          <select
            value={moveTarget}
            disabled={busy}
            onChange={(event) => setMoveTarget(event.target.value)}
          >
            <option value="">（移到根分类）</option>
            {moveOptions.map((item) => (
              <option key={item.path} value={item.path}>
                {`${"— ".repeat(item.depth)}${item.path}`}
              </option>
            ))}
          </select>
          <button
            type="button"
            className="btn btn-primary btn-compact"
            disabled={busy || moveTarget === (parent?.path ?? "")}
            onClick={() => void move()}
          >
            移动到这里
          </button>
          <button
            type="button"
            className="btn btn-quiet btn-compact"
            disabled={busy}
            onClick={() => setAction(null)}
          >
            取消
          </button>
        </div>
      )}

      {children.map((child) => (
        <CategoryNode
          key={child.path}
          category={child}
          categories={categories}
          counts={counts}
          onChanged={onChanged}
          onNotify={onNotify}
        />
      ))}
    </div>
  );
}
