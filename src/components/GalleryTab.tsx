import { useCallback, useEffect, useState } from "react";
import { api } from "../api";
import type { ArchiveCardData, Collection } from "../types";
import type { ToastData, ToastKind } from "./Toast";
import Toast from "./Toast";
import Reader from "./Reader";
import DetailDrawer from "./DetailDrawer";

/**
 * 画廊单开标签的内容：按 archiveId 独立加载画集，直接渲染 Reader。
 *
 * 与主页的 `readerCard` 完全解耦——每个 gallery 标签一个 Reader 实例，
 * 关标签 = 调 Reader 的 onClose（内部已写进度），主页不受影响。
 *
 * 2026-11-27 管理员验收反馈「点进画廊了没法开详情编辑」：gallery 标签里的
 * Reader 原先没接 onOpenDetail → 详情按钮/浮动按钮/右键菜单全都不渲染。
 * 现在本组件自挂 DetailDrawer 并实现编辑回调（评分/收藏夹/封面/备注/删除），
 * 回调只改本标签局部状态 + 写库，不惊动主页。
 */
export default function GalleryTab({
  archiveId,
  onClose,
}: {
  archiveId: number;
  onClose: () => void;
}) {
  const [card, setCard] = useState<ArchiveCardData | null>(null);
  const [collections, setCollections] = useState<Collection[]>([]);
  const [detailOpen, setDetailOpen] = useState(false);
  const [toast, setToast] = useState<ToastData | null>(null);
  const [error, setError] = useState("");

  const notify = useCallback((message: string, kind: ToastKind = "info") => {
    setToast({ id: Date.now(), message, kind });
  }, []);

  const reloadCard = useCallback(async () => {
    try {
      const page = await api.queryArchives({ archiveIds: [archiveId], pageSize: 12 });
      const found = page.items.find((item) => item.id === archiveId) ?? page.items[0] ?? null;
      if (found) setCard(found);
      else setError("画集不存在或已删除");
    } catch (err) {
      setError(String(err));
    }
  }, [archiveId]);

  useEffect(() => {
    void reloadCard();
  }, [reloadCard]);

  // 详情编辑需要收藏夹列表（成员勾选/设封面）。
  useEffect(() => {
    let cancelled = false;
    void api
      .listCollections()
      .then((list) => {
        if (!cancelled) setCollections(list);
      })
      .catch(() => {
        /* 拉不到就空列表：详情仍可看，只是收藏夹勾选为空 */
      });
    return () => {
      cancelled = true;
    };
  }, []);

  if (error) {
    return (
      <div className="gallery-tab-state">
        <p>{error}</p>
        <button type="button" className="btn btn-quiet btn-compact" onClick={onClose}>
          关闭标签
        </button>
      </div>
    );
  }

  if (!card) {
    return <div className="gallery-tab-state">正在打开画集…</div>;
  }

  return (
    <div className="gallery-tab">
      <Reader
        card={card}
        onClose={onClose}
        onNotify={notify}
        enterMode="normal"
        onOpenDetail={() => setDetailOpen(true)}
        detailOpen={detailOpen}
        onCloseDetail={() => setDetailOpen(false)}
      />

      {detailOpen && (
        <DetailDrawer
          card={card}
          collections={collections}
          embeddedInReader
          dock="float"
          onClose={() => setDetailOpen(false)}
          onRead={() => setDetailOpen(false)}
          onNotify={notify}
          onRatingChange={async (id, rating) => {
            await api.setRating(id, rating);
            setCard((current) => (current && current.id === id ? { ...current, rating } : current));
          }}
          onCollectionToggle={async (id, collectionId, member) => {
            await api.setArchiveCollection(id, collectionId, member);
            setCollections(await api.listCollections());
          }}
          onCollectionCover={async (collectionId, id) => {
            await api.setCollectionCover(collectionId, id);
            setCollections(await api.listCollections());
          }}
          onNoteChange={(id, note) => {
            setCard((current) => (current && current.id === id ? { ...current, note } : current));
          }}
          onTagsChanged={() => reloadCard()}
          onDelete={async (id) => {
            await api.deleteArchive(id);
            setDetailOpen(false);
            onClose();
          }}
        />
      )}

      {toast && <Toast toast={toast} onClose={() => setToast(null)} />}
    </div>
  );
}
