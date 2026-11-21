import { useCallback, useEffect, useState } from "react";
import { api } from "../api";
import type { LooseImage, LooseImageCollection } from "../types";
import {
  CloseIcon,
  FolderIcon,
  PlusIcon,
  RefreshIcon,
  StarIcon,
  TrashIcon,
} from "./Icons";

interface LoosePanelProps {
  /** 弹层形态的关闭回调；整页形态（variant="page"）不需要传。 */
  onClose?: () => void;
  onNotify: (message: string, kind?: "info" | "success" | "error") => void;
  /**
   * modal = 盖在画廊上的弹层（旧形态，保留兼容）；
   * page = 作为主导航的主内容区整页渲染（侧栏「散图收藏夹」点进来就是这个）。
   */
  variant?: "modal" | "page";
}

const DEFAULT_RATING_STEP = 10;

/**
 * 散图收藏夹面板。
 *
 * 区 = 磁盘上的一个真实文件夹；记录只引用路径/画廊源页，不迁移文件。
 * 两种记录：file（复制粘贴进文件夹，扫进入库）、archive_page（从画廊收藏的源页）。
 */
export default function LoosePanel({
  onClose,
  onNotify,
  variant = "modal",
}: LoosePanelProps) {
  const isPage = variant === "page";
  const [collections, setCollections] = useState<LooseImageCollection[]>([]);
  const [activeId, setActiveId] = useState<number | null>(null);
  const [images, setImages] = useState<LooseImage[]>([]);
  const [loading, setLoading] = useState(true);
  const [scanning, setScanning] = useState(false);
  const [createDraft, setCreateDraft] = useState("");
  const [createBusy, setCreateBusy] = useState(false);
  const [confirmingDelete, setConfirmingDelete] = useState<number | null>(null);
  const [viewIndex, setViewIndex] = useState<number | null>(null);

  const loadCollections = useCallback(async () => {
    const list = await api.looseListCollections();
    setCollections(list);
    return list;
  }, []);

  const loadImages = useCallback(async (collectionId: number) => {
    setImages(await api.looseListImages(collectionId));
  }, []);

  const selectCollection = useCallback(
    async (id: number) => {
      setActiveId(id);
      setLoading(true);
      try {
        await loadImages(id);
      } catch (error) {
        onNotify(`读取散图失败：${String(error)}`, "error");
      } finally {
        setLoading(false);
      }
    },
    [loadImages, onNotify],
  );

  const refreshAll = useCallback(async () => {
    const list = await loadCollections();
    const target = list.find((collection) => collection.id === activeId) ?? list[0];
    if (target) {
      await selectCollection(target.id);
    } else {
      setActiveId(null);
      setImages([]);
    }
  }, [activeId, loadCollections, loadImages, selectCollection]);

  useEffect(() => {
    void (async () => {
      setLoading(true);
      try {
        const list = await loadCollections();
        if (list[0]) {
          await loadImages(list[0].id);
          setActiveId(list[0].id);
        }
      } catch (error) {
        onNotify(`读取散图区失败：${String(error)}`, "error");
      } finally {
        setLoading(false);
      }
    })();
  }, [loadCollections, loadImages, onNotify]);

  async function createCollection() {
    const name = createDraft.trim();
    if (!name) {
      onNotify("先给散图区起个名字", "error");
      return;
    }
    if (createBusy) return;
    setCreateBusy(true);
    try {
      const created = await api.looseCreateCollection(name);
      setCreateDraft("");
      onNotify(
        `已创建「${created.name}」——把图片丢进文件夹后点「重新扫描」`,
        "success",
      );
      await loadCollections();
      await selectCollection(created.id);
    } catch (error) {
      onNotify(`创建失败：${String(error)}`, "error");
    } finally {
      setCreateBusy(false);
    }
  }

  async function scan() {
    if (!activeId || scanning) return;
    setScanning(true);
    try {
      const [added, missing, total] = await api.looseScanCollection(activeId);
      await loadImages(activeId);
      await loadCollections();
      onNotify(
        `扫描完成：新增 ${added}，缺失 ${missing}，现有 ${total} 张`,
        added > 0 ? "success" : "info",
      );
    } catch (error) {
      onNotify(`扫描失败：${String(error)}`, "error");
    } finally {
      setScanning(false);
    }
  }

  async function openFolder() {
    const active = collections.find((collection) => collection.id === activeId);
    if (!active) {
      onNotify("先选择一个散图区", "error");
      return;
    }
    try {
      await api.looseOpenFolder(active.folder);
    } catch (error) {
      onNotify(`打开文件夹失败：${String(error)}`, "error");
    }
  }

  async function removeCollection(collection: LooseImageCollection) {
    if (confirmingDelete !== collection.id) {
      setConfirmingDelete(collection.id);
      window.setTimeout(
        () => setConfirmingDelete((current) => (current === collection.id ? null : current)),
        3200,
      );
      return;
    }
    setConfirmingDelete(null);
    try {
      await api.looseDeleteCollection(collection.id);
      onNotify(
        `已删除「${collection.name}」的记录——磁盘文件夹与图片原样保留`,
        "success",
      );
      setActiveId(null);
      setImages([]);
      await loadCollections();
    } catch (error) {
      onNotify(`删除失败：${String(error)}`, "error");
    }
  }

  async function removeImage(image: LooseImage) {
    try {
      // 只移除记录，不动磁盘文件（文件可能是管理员自己放的）
      await api.looseRemoveImage(image.id, false);
      setImages((previous) => previous.filter((item) => item.id !== image.id));
      if (viewIndex !== null) {
        if (images.length <= 1) setViewIndex(null);
        else if (viewIndex >= images.length - 1) setViewIndex(images.length - 2);
      }
      onNotify("已移除记录（磁盘文件未动）", "success");
    } catch (error) {
      onNotify(`移除失败：${String(error)}`, "error");
    }
  }

  async function setRating(image: LooseImage, rating: number) {
    const next = Math.min(100, Math.max(0, rating));
    setImages((previous) =>
      previous.map((item) => (item.id === image.id ? { ...item, rating: next } : item)),
    );
    try {
      await api.looseSetRating(image.id, next);
    } catch (error) {
      onNotify(`评分保存失败：${String(error)}`, "error");
    }
  }

  async function setFavorite(image: LooseImage) {
    const next = !image.favorite;
    setImages((previous) =>
      previous.map((item) => (item.id === image.id ? { ...item, favorite: next } : item)),
    );
    try {
      await api.looseSetFavorite(image.id, next);
    } catch (error) {
      onNotify(`收藏标记失败：${String(error)}`, "error");
    }
  }

  const active = collections.find((collection) => collection.id === activeId) ?? null;
  const viewing = viewIndex !== null ? images[viewIndex] ?? null : null;

  // 查看器键盘：左右切换、Esc 关
  useEffect(() => {
    if (viewIndex === null) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setViewIndex(null);
      else if (event.key === "ArrowLeft" && viewIndex > 0) setViewIndex(viewIndex - 1);
      else if (event.key === "ArrowRight" && viewIndex < images.length - 1) {
        setViewIndex(viewIndex + 1);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [viewIndex, images.length]);

  const inner = (
    <div
      className={isPage ? "loose-page" : "queue-panel loose-panel"}
      role={isPage ? undefined : "dialog"}
      aria-modal={isPage ? undefined : true}
      aria-label="散图收藏夹"
      onClick={isPage ? undefined : (event) => event.stopPropagation()}
    >
        <header className="queue-panel-head">
          <div>
            <p className="eyebrow">LOOSE IMAGES</p>
            <h2>散图收藏夹</h2>
          </div>
          <div className="queue-panel-head-actions">
            <button
              type="button"
              className="icon-button"
              onClick={() => void refreshAll()}
              aria-label="刷新"
              title="刷新"
            >
              <RefreshIcon width={16} height={16} />
            </button>
            {onClose && (
              <button type="button" className="icon-button" onClick={onClose} aria-label="关闭">
                <CloseIcon width={16} height={16} />
              </button>
            )}
          </div>
        </header>

        <div className={isPage ? "loose-page-body" : "queue-panel-body"}>
          <section className="queue-generate">
            <div className="queue-generate-row">
              <div>
                <strong>新建散图区</strong>
                <p className="queue-hint">
                  每个区是一个真实文件夹（默认在库旁「散图」目录下）。把图片丢进去，
                  点「重新扫描」登记——画匣只记录路径，不迁移不复制。
                </p>
              </div>
              <div style={{ display: "flex", gap: 6 }}>
                <input
                  value={createDraft}
                  onChange={(event) => setCreateDraft(event.target.value)}
                  onKeyDown={(event) => {
                    if (event.key === "Enter") void createCollection();
                  }}
                  placeholder="区名…"
                  aria-label="散图区名字"
                  style={{
                    width: 150,
                    padding: "7px 10px",
                    borderRadius: 6,
                    border: "1px solid var(--border-strong)",
                    background: "var(--panel-raised)",
                    color: "var(--text)",
                  }}
                />
                <button
                  type="button"
                  className="btn btn-quiet btn-compact"
                  onClick={() => void createCollection()}
                  disabled={createBusy || !createDraft.trim()}
                >
                  <PlusIcon width={14} height={14} />
                  {createBusy ? "创建中…" : "新建"}
                </button>
              </div>
            </div>
            <div className="queue-generate-row" style={{ flexWrap: "wrap", gap: 6 }}>
              {(collections.length > 0
                ? collections
                : []
              ).map((collection) => (
                <button
                  key={collection.id}
                  type="button"
                  className={`queue-tag-pick${collection.id === activeId ? " is-active" : ""}`}
                  onClick={() => void selectCollection(collection.id)}
                  title={`${collection.folder}（${collection.count} 张 / 收藏 ${collection.favorites}）`}
                >
                  {collection.name}
                  <span className="queue-tag-count">{collection.count}</span>
                </button>
              ))}
              {collections.length === 0 && !loading && (
                <span className="queue-hint">还没有散图区——上面建一个</span>
              )}
            </div>
            {active && (
              <div className="queue-generate-row" style={{ flexWrap: "wrap", gap: 6 }}>
                <button
                  type="button"
                  className="btn btn-quiet btn-compact"
                  onClick={() => void scan()}
                  disabled={scanning}
                  title="登记文件夹里新增的图片，检测缺失"
                >
                  <RefreshIcon width={14} height={14} />
                  {scanning ? "扫描中…" : "重新扫描"}
                </button>
                <button
                  type="button"
                  className="btn btn-quiet btn-compact"
                  onClick={() => void openFolder()}
                  title="在资源管理器打开这个区的文件夹（复制粘贴图片用）"
                >
                  <FolderIcon width={14} height={14} />
                  打开文件夹
                </button>
                <button
                  type="button"
                  className={`btn btn-compact${
                    confirmingDelete === active.id ? " btn-danger" : " btn-quiet"
                  }`}
                  onClick={() => void removeCollection(active)}
                  title="只删除记录；磁盘文件夹与图片原样保留"
                >
                  <TrashIcon width={14} height={14} />
                  {confirmingDelete === active.id ? "确认删除" : "删除此区"}
                </button>
                <span className="queue-hint">
                  {active.folder} · {active.count} 张 · 收藏 {active.favorites}
                </span>
              </div>
            )}
          </section>

          <section className="loose-grid-wrap">
            {loading && <p className="queue-hint">读取中…</p>}
            {!loading && images.length === 0 && (
              <p className="queue-hint">
                这个区还是空的——点「打开文件夹」把图片复制进去，回来「重新扫描」。
                从画廊收藏的页也会出现在这里。
              </p>
            )}
            {images.length > 0 && (
              <div className="loose-grid">
                {images.map((image, index) => (
                  <div className="loose-card" key={image.id}>
                    <button
                      type="button"
                      className="loose-thumb"
                      onClick={() => setViewIndex(index)}
                      title={image.path ?? `源自画集 P${(image.pageIndex ?? 0) + 1}`}
                    >
                      {image.missing ? (
                        <span className="loose-missing">文件缺失</span>
                      ) : (
                        <img
                          src={api.looseImageUrl("thumb", image.id)}
                          alt=""
                          loading="lazy"
                          decoding="async"
                          draggable={false}
                        />
                      )}
                      {image.kind === "archive_page" && (
                        <span className="loose-src-badge">
                          源自：{image.archiveTitle ?? "画集"}
                        </span>
                      )}
                      {image.favorite && <span className="loose-fav">♥</span>}
                      {image.rating > 0 && (
                        <span className="loose-rating">
                          <StarIcon width={10} height={10} />
                          {image.rating}
                        </span>
                      )}
                    </button>
                    <button
                      type="button"
                      className="loose-remove"
                      title="移除记录（不动磁盘文件）"
                      aria-label="移除记录"
                      onClick={() => void removeImage(image)}
                    >
                      ×
                    </button>
                  </div>
                ))}
              </div>
            )}
          </section>
        </div>

        {viewing && (
          <div
            className="loose-viewer"
            role="dialog"
            aria-modal="true"
            onClick={() => setViewIndex(null)}
          >
            <div
              className="loose-viewer-body"
              onClick={(event) => event.stopPropagation()}
            >
              <img src={api.looseImageUrl("img", viewing.id)} alt="" draggable={false} />
              <div className="loose-viewer-bar">
                <span className="queue-hint">
                  {viewIndex !== null ? `${viewIndex + 1} / ${images.length}` : ""}
                </span>
                {viewing.kind === "archive_page" && (
                  <span className="queue-hint">
                    源自：{viewing.archiveTitle ?? "画集"} P{(viewing.pageIndex ?? 0) + 1}
                  </span>
                )}
                <span style={{ flex: 1 }} />
                <button
                  type="button"
                  className="btn btn-quiet btn-compact"
                  onClick={() => void setRating(viewing, viewing.rating - DEFAULT_RATING_STEP)}
                  title="评分 −10"
                >
                  −
                </button>
                <span className="loose-rating" style={{ padding: "0 6px" }}>
                  <StarIcon width={12} height={12} />
                  {viewing.rating}
                </span>
                <button
                  type="button"
                  className="btn btn-quiet btn-compact"
                  onClick={() => void setRating(viewing, viewing.rating + DEFAULT_RATING_STEP)}
                  title="评分 +10"
                >
                  +
                </button>
                <button
                  type="button"
                  className={`btn btn-compact${viewing.favorite ? " btn-danger" : " btn-quiet"}`}
                  onClick={() => void setFavorite(viewing)}
                  title="收藏（红心）"
                >
                  {viewing.favorite ? "♥ 已收藏" : "♡ 收藏"}
                </button>
                <button
                  type="button"
                  className="btn btn-quiet btn-compact"
                  onClick={() => {
                    void removeImage(viewing);
                    setViewIndex(null);
                  }}
                >
                  移除记录
                </button>
                <button
                  type="button"
                  className="btn btn-quiet btn-compact"
                  onClick={() => setViewIndex(null)}
                >
                  关闭
                </button>
              </div>
            </div>
          </div>
        )}
    </div>
  );

  // 整页形态直接铺进主内容区；弹层形态再套一层遮罩（旧入口兼容）。
  if (isPage) return inner;
  return (
    <div className="modal-backdrop" role="presentation" onClick={() => onClose?.()}>
      {inner}
    </div>
  );
}
