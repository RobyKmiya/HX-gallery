import { useEffect, useState } from "react";
import { api, assetUrl } from "../api";
import type { ArchiveCardData, Collection } from "../types";
import { COLLECTION_NAME_PRESETS } from "./CollectionNameDialog";
import {
  CheckIcon,
  CloseIcon,
  HeartIcon,
  LogoIcon,
  PlusIcon,
  StarIcon,
} from "./Icons";

interface CollectionEditorProps {
  card: ArchiveCardData;
  collections: Collection[];
  onClose: () => void;
  onOpenDetail: () => void;
  onChanged: () => Promise<void>;
  onNotify: (message: string, kind?: "info" | "success" | "error") => void;
}

export default function CollectionEditor({
  card,
  collections,
  onClose,
  onOpenDetail,
  onChanged,
  onNotify,
}: CollectionEditorProps) {
  const [memberIds, setMemberIds] = useState<number[]>([]);
  const [loading, setLoading] = useState(true);
  const [busyId, setBusyId] = useState<number | null>(null);
  const [coverBusyId, setCoverBusyId] = useState<number | null>(null);
  const [newName, setNewName] = useState("");
  const [creating, setCreating] = useState(false);
  const [imageFailed, setImageFailed] = useState(false);

  const cover =
    card.coverPath && !imageFailed ? assetUrl(card.coverPath) : null;

  useEffect(() => {
    setImageFailed(false);
  }, [card.coverPath]);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setMemberIds([]);

    api
      .listArchiveCollections(card.id)
      .then((ids) => {
        if (!cancelled) setMemberIds(ids);
      })
      .catch((error) => {
        if (!cancelled) {
          onNotify(`收藏夹加载失败：${String(error)}`, "error");
        }
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });

    return () => {
      cancelled = true;
    };
  }, [card.id, onNotify]);

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [onClose]);

  async function toggle(collection: Collection) {
    if (busyId !== null) return;
    const member = !memberIds.includes(collection.id);
    setBusyId(collection.id);
    try {
      await api.setArchiveCollection(card.id, collection.id, member);
      setMemberIds((previous) =>
        member
          ? [...previous, collection.id]
          : previous.filter((id) => id !== collection.id),
      );
      await onChanged();
      onNotify(
        member
          ? `已加入「${collection.name}」`
          : `已移出「${collection.name}」`,
        "success",
      );
    } catch (error) {
      onNotify(`收藏夹操作失败：${String(error)}`, "error");
    } finally {
      setBusyId(null);
    }
  }

  async function toggleCover(collection: Collection) {
    if (coverBusyId !== null) return;
    const isCover = collection.coverArchiveId === card.id;
    setCoverBusyId(collection.id);
    try {
      await api.setCollectionCover(
        collection.id,
        isCover ? null : card.id,
      );
      await onChanged();
      onNotify(
        isCover
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

  async function createAndAdd() {
    const name = newName.trim();
    if (!name || creating) return;
    setCreating(true);
    try {
      const created = await api.createCollection(name);
      await api.setArchiveCollection(card.id, created.id, true);
      setMemberIds((previous) => [...previous, created.id]);
      setNewName("");
      await onChanged();
      onNotify(`已新建「${created.name}」并加入`, "success");
    } catch (error) {
      onNotify(`新建收藏夹失败：${String(error)}`, "error");
    } finally {
      setCreating(false);
    }
  }

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="collection-editor"
        role="dialog"
        aria-modal="true"
        aria-label={`管理 ${card.title} 的收藏夹`}
        onClick={(event) => event.stopPropagation()}
      >
        <header className="collection-editor-head">
          <div className="collection-editor-info">
            <span className="collection-editor-cover">
              {cover ? (
                <img src={cover} alt="" onError={() => setImageFailed(true)} />
              ) : (
                <LogoIcon width={22} height={22} />
              )}
            </span>
            <div>
              <p className="eyebrow">COLLECTIONS</p>
              <h2 title={card.title}>{card.title}</h2>
              <p>
                {loading
                  ? "正在读取收藏夹…"
                  : `选择收藏位置 · ${memberIds.length} 个收藏夹 · 勾选即保存`}
              </p>
            </div>
          </div>
          <button
            type="button"
            className="icon-button"
            onClick={onClose}
            aria-label="关闭"
          >
            <CloseIcon width={16} height={16} />
          </button>
        </header>

        <div className="collection-editor-list">
          {loading ? (
            <div className="collection-editor-empty">
              <span className="spinner" />
            </div>
          ) : collections.length === 0 ? (
            <div className="collection-editor-empty">
              <HeartIcon width={20} height={20} />
              <p>还没有收藏夹，先在下方新建一个。</p>
            </div>
          ) : (
            collections.map((collection) => {
              const active = memberIds.includes(collection.id);
              const isCover = collection.coverArchiveId === card.id;
              return (
                <div className="collection-row" key={collection.id}>
                  <button
                    type="button"
                    className={`collection-toggle${active ? " is-active" : ""}`}
                    onClick={() => void toggle(collection)}
                    disabled={busyId !== null}
                  >
                    <span className="collection-check">
                      {active && <CheckIcon width={12} height={12} />}
                    </span>
                    <span className="collection-name">{collection.name}</span>
                    <span className="collection-count">
                      {collection.count}
                    </span>
                  </button>
                  <button
                    type="button"
                    className={`collection-cover-btn${isCover ? " is-cover" : ""}`}
                    onClick={() => void toggleCover(collection)}
                    disabled={coverBusyId !== null}
                    title={
                      isCover
                        ? "取消该收藏夹的封面"
                        : "把当前画集设为该收藏夹封面"
                    }
                  >
                    <StarIcon width={14} height={14} />
                  </button>
                </div>
              );
            })
          )}
        </div>

        <form
          className="collection-editor-create"
          onSubmit={(event) => {
            event.preventDefault();
            void createAndAdd();
          }}
        >
          <div className="collection-editor-create-row">
            <input
              value={newName}
              maxLength={24}
              placeholder="新建收藏夹并收藏…"
              disabled={creating}
              onChange={(event) => setNewName(event.target.value)}
            />
            <button
              type="submit"
              className="btn btn-ghost btn-compact"
              disabled={creating || !newName.trim()}
            >
              <PlusIcon width={13} height={13} />
              {creating ? "创建中…" : "新建"}
            </button>
          </div>
          <div className="collection-editor-presets">
            <span>常用</span>
            {COLLECTION_NAME_PRESETS.map((preset) => (
              <button
                key={preset}
                type="button"
                className={`filter-chip${newName.trim() === preset ? " is-active" : ""}`}
                onClick={() => setNewName(preset)}
                disabled={creating}
              >
                {preset}
              </button>
            ))}
          </div>
        </form>

        <footer className="collection-editor-foot">
          <span>星标 = 该收藏夹的封面</span>
          <div>
            <button
              type="button"
              className="btn btn-ghost btn-compact"
              onClick={onOpenDetail}
            >
              打开完整详情
            </button>
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
