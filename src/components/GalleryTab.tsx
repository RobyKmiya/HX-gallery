import { useEffect, useState } from "react";
import { api } from "../api";
import type { ArchiveCardData } from "../types";
import Reader from "./Reader";

/**
 * 画廊单开标签的内容：按 archiveId 独立加载画集，直接渲染 Reader。
 *
 * 与主页的 `readerCard` 完全解耦——每个 gallery 标签一个 Reader 实例，
 * 关标签 = 调 Reader 的 onClose（内部已写进度），主页不受影响。
 */
export default function GalleryTab({
  archiveId,
  onClose,
}: {
  archiveId: number;
  onClose: () => void;
}) {
  const [card, setCard] = useState<ArchiveCardData | null>(null);
  const [error, setError] = useState("");

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const page = await api.queryArchives({ archiveIds: [archiveId], pageSize: 12 });
        const found = page.items.find((item) => item.id === archiveId) ?? page.items[0] ?? null;
        if (cancelled) return;
        if (found) setCard(found);
        else setError("画集不存在或已删除");
      } catch (err) {
        if (!cancelled) setError(String(err));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [archiveId]);

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
      <Reader card={card} onClose={onClose} onNotify={() => {}} enterMode="normal" />
    </div>
  );
}
