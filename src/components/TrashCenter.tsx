import { useCallback, useEffect, useState } from "react";
import { api, assetUrl } from "../api";
import { formatBytes, formatDateTime } from "../format";
import type { Collection, Tag, TrashedArchive } from "../types";
import { CheckIcon, CloseIcon, LogoIcon, TrashIcon } from "./Icons";

type TrashTab = "archives" | "collections" | "tags";

interface TrashCenterProps {
  onClose: () => void;
  onChanged: () => Promise<void>;
  onNotify: (message: string, kind?: "info" | "success" | "error") => void;
}

const TAB_LABELS: Record<TrashTab, string> = {
  archives: "画集",
  collections: "收藏夹",
  tags: "标签",
};

export default function TrashCenter({
  onClose,
  onChanged,
  onNotify,
}: TrashCenterProps) {
  const [tab, setTab] = useState<TrashTab>("archives");
  const [archives, setArchives] = useState<TrashedArchive[]>([]);
  const [collections, setCollections] = useState<Collection[]>([]);
  const [tags, setTags] = useState<Tag[]>([]);
  const [loading, setLoading] = useState(true);
  const [busyKey, setBusyKey] = useState<string | null>(null);
  const [pendingPurgeKey, setPendingPurgeKey] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const [archiveItems, collectionItems, tagItems] = await Promise.all([
        api.listDeletedArchives(),
        api.listDeletedCollections(),
        api.listDeletedTags(),
      ]);
      setArchives(archiveItems);
      setCollections(collectionItems);
      setTags(tagItems);
    } catch (error) {
      onNotify(`回收站加载失败：${String(error)}`, "error");
    } finally {
      setLoading(false);
    }
  }, [onNotify]);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [onClose]);

  function confirmPurge(key: string): boolean {
    if (pendingPurgeKey !== key) {
      setPendingPurgeKey(key);
      window.setTimeout(() => {
        setPendingPurgeKey((current) => (current === key ? null : current));
      }, 3200);
      return false;
    }
    setPendingPurgeKey(null);
    return true;
  }

  async function restoreArchive(item: TrashedArchive) {
    if (busyKey !== null) return;
    setBusyKey(`archives-${item.id}`);
    try {
      await api.restoreArchive(item.id);
      await onChanged();
      await load();
      onNotify(
        item.missing
          ? `已恢复「${item.title}」，但文件当前缺失`
          : `已恢复画集「${item.title}」`,
        item.missing ? "info" : "success",
      );
    } catch (error) {
      onNotify(`恢复失败：${String(error)}`, "error");
    } finally {
      setBusyKey(null);
    }
  }

  async function purgeArchive(item: TrashedArchive) {
    if (busyKey !== null) return;
    if (!confirmPurge(`archives-${item.id}`)) return;
    setBusyKey(`archives-${item.id}`);
    try {
      await api.purgeArchive(item.id);
      await load();
      onNotify(`已从库中彻底移除「${item.title}」`, "success");
    } catch (error) {
      onNotify(`彻底删除失败：${String(error)}`, "error");
    } finally {
      setBusyKey(null);
    }
  }

  async function restoreCollection(collection: Collection) {
    if (busyKey !== null) return;
    setBusyKey(`collections-${collection.id}`);
    try {
      await api.restoreCollection(collection.id);
      await onChanged();
      await load();
      onNotify(`已恢复收藏夹「${collection.name}」`, "success");
    } catch (error) {
      onNotify(`恢复失败：${String(error)}`, "error");
    } finally {
      setBusyKey(null);
    }
  }

  async function purgeCollection(collection: Collection) {
    if (busyKey !== null) return;
    if (!confirmPurge(`collections-${collection.id}`)) return;
    setBusyKey(`collections-${collection.id}`);
    try {
      await api.purgeCollection(collection.id);
      await load();
      onNotify(`已彻底删除收藏夹「${collection.name}」`, "success");
    } catch (error) {
      onNotify(`彻底删除失败：${String(error)}`, "error");
    } finally {
      setBusyKey(null);
    }
  }

  async function restoreTag(tag: Tag) {
    if (busyKey !== null) return;
    setBusyKey(`tags-${tag.id}`);
    try {
      await api.restoreTag(tag.id);
      await onChanged();
      await load();
      onNotify(`已恢复标签「${tag.translatedName || tag.name}」`, "success");
    } catch (error) {
      onNotify(`恢复失败：${String(error)}`, "error");
    } finally {
      setBusyKey(null);
    }
  }

  async function purgeTag(tag: Tag) {
    if (busyKey !== null) return;
    if (!confirmPurge(`tags-${tag.id}`)) return;
    setBusyKey(`tags-${tag.id}`);
    try {
      await api.purgeTag(tag.id);
      await load();
      onNotify(`已彻底删除标签「${tag.translatedName || tag.name}」`, "success");
    } catch (error) {
      onNotify(`彻底删除失败：${String(error)}`, "error");
    } finally {
      setBusyKey(null);
    }
  }

  function renderActions(key: string, onRestore: () => void, onPurge: () => void) {
    const confirming = pendingPurgeKey === key;
    const busy = busyKey === key;
    return (
      <div className="trash-row-actions">
        <button
          type="button"
          className="btn btn-ghost btn-compact"
          disabled={busyKey !== null}
          onClick={onRestore}
        >
          <CheckIcon width={12} height={12} />
          恢复
        </button>
        <button
          type="button"
          className={`btn btn-compact${
            confirming ? " btn-danger" : " btn-quiet"
          }`}
          disabled={busyKey !== null}
          onClick={onPurge}
          title={
            confirming ? "再次点击确认彻底删除" : "从数据库彻底删除，不可恢复"
          }
        >
          {busy ? "处理中…" : confirming ? "确认删除" : "彻底删除"}
        </button>
      </div>
    );
  }

  const total = archives.length + collections.length + tags.length;
  const tabCounts: Record<TrashTab, number> = {
    archives: archives.length,
    collections: collections.length,
    tags: tags.length,
  };

  const hints: Record<TrashTab, string> = {
    archives:
      "移入回收站不会删除原始文件；恢复后收藏夹、标签、阅读进度与书签一并回来。彻底删除只移除库记录和封面缓存。",
    collections:
      "删除的收藏夹会保留画集关联，恢复后原样回来；彻底删除不可撤销，但不会删除画集文件。",
    tags: "删除标签会保留画集关联，恢复后重新生效；已合并的标签不能直接恢复。",
  };

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="collection-editor collection-trash trash-center"
        role="dialog"
        aria-modal="true"
        aria-label="回收站"
        onClick={(event) => event.stopPropagation()}
      >
        <header className="collection-editor-head">
          <div className="collection-editor-info">
            <span className="collection-editor-cover">
              <TrashIcon width={22} height={22} />
            </span>
            <div>
              <p className="eyebrow">TRASH</p>
              <h2>回收站</h2>
              <p>
                {loading
                  ? "正在读取回收站…"
                  : total === 0
                    ? "回收站是空的"
                    : `${archives.length} 个画集 · ${collections.length} 个收藏夹 · ${tags.length} 个标签`}
              </p>
            </div>
          </div>
          <button
            type="button"
            className="icon-button"
            onClick={onClose}
            aria-label="关闭回收站"
          >
            <CloseIcon width={16} height={16} />
          </button>
        </header>

        <div className="trash-tabs" role="tablist" aria-label="回收站分类">
          {(Object.keys(TAB_LABELS) as TrashTab[]).map((key) => (
            <button
              key={key}
              type="button"
              role="tab"
              aria-selected={tab === key}
              className={`trash-tab${tab === key ? " is-active" : ""}`}
              onClick={() => setTab(key)}
            >
              {TAB_LABELS[key]}
              <span>{tabCounts[key]}</span>
            </button>
          ))}
        </div>

        <p className="trash-hint">{hints[tab]}</p>

        <div className="collection-editor-list trash-list">
          {loading ? (
            <div className="collection-editor-empty">
              <span className="spinner" />
            </div>
          ) : tab === "archives" ? (
            archives.length === 0 ? (
              <div className="collection-editor-empty">
                <TrashIcon width={20} height={20} />
                <p>没有已删除的画集。</p>
              </div>
            ) : (
              archives.map((item) => (
                <div className="trash-row" key={item.id}>
                  <div className="trash-row-main trash-archive-main">
                    <span className="trash-cover">
                      {item.coverPath ? (
                        <img src={assetUrl(item.coverPath)} alt="" loading="lazy" />
                      ) : (
                        <LogoIcon width={18} height={18} />
                      )}
                    </span>
                    <div className="trash-archive-info">
                      <strong title={item.fileName}>{item.title}</strong>
                      <span>
                        {item.fileName} · {item.pageCount} 页 ·{" "}
                        {formatBytes(item.fileSize)}
                        {item.missing ? " · 文件缺失" : ""}
                      </span>
                      <span>删除于 {formatDateTime(item.deletedAt)}</span>
                    </div>
                  </div>
                  {renderActions(
                    `archives-${item.id}`,
                    () => void restoreArchive(item),
                    () => void purgeArchive(item),
                  )}
                </div>
              ))
            )
          ) : tab === "collections" ? (
            collections.length === 0 ? (
              <div className="collection-editor-empty">
                <TrashIcon width={20} height={20} />
                <p>没有已删除的收藏夹。</p>
              </div>
            ) : (
              collections.map((collection) => (
                <div className="trash-row" key={collection.id}>
                  <div className="trash-row-main">
                    <strong title={collection.name}>{collection.name}</strong>
                    <span>
                      {collection.count} 个画集 · 删除于{" "}
                      {formatDateTime(collection.deletedAt ?? 0)}
                    </span>
                  </div>
                  {renderActions(
                    `collections-${collection.id}`,
                    () => void restoreCollection(collection),
                    () => void purgeCollection(collection),
                  )}
                </div>
              ))
            )
          ) : tags.length === 0 ? (
            <div className="collection-editor-empty">
              <TrashIcon width={20} height={20} />
              <p>没有已删除的标签。</p>
            </div>
          ) : (
            tags.map((tag) => (
              <div className="trash-row" key={tag.id}>
                <div className="trash-row-main">
                  <strong title={tag.name}>
                    #{tag.number} {tag.translatedName || tag.name}
                  </strong>
                  <span>
                    {tag.count} 个画集 · 删除于{" "}
                    {formatDateTime(tag.deletedAt ?? 0)}
                  </span>
                </div>
                {renderActions(
                  `tags-${tag.id}`,
                  () => void restoreTag(tag),
                  () => void purgeTag(tag),
                )}
              </div>
            ))
          )}
        </div>

        <footer className="collection-editor-foot">
          <span>恢复后回到原来的位置；彻底删除不可撤销</span>
          <div>
            <button
              type="button"
              className="btn btn-primary btn-compact"
              onClick={onClose}
            >
              完成
            </button>
          </div>
        </footer>
      </div>
    </div>
  );
}
