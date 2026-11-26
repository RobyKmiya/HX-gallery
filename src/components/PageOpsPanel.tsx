import { useCallback, useEffect, useState } from "react";
import { emit, listen } from "@tauri-apps/api/event";
import { api } from "../api";
import { ImportIcon } from "./Icons";

/**
 * 图片管理面板（zip/cbz 单页增删，重打包在后台线程）。
 *
 * 详情页和阅读器共用：详情页折叠在「图片管理」区里，阅读器从工具栏弹出。
 * 完成增删后发 `d2://download` 让主页网格防抖刷新（页数变了卡片要跟着变）。
 */
export default function PageOpsPanel({
  archiveId,
  onNotify,
}: {
  archiveId: number;
  onNotify: (message: string, kind?: "info" | "success" | "error") => void;
}) {
  const [pageList, setPageList] = useState<{
    kind: string;
    encrypted: boolean;
    pages: { index: number; name: string; size: number }[];
    pageCount: number;
  } | null>(null);
  const [pageOpsBusy, setPageOpsBusy] = useState("");
  const [pageProgress, setPageProgress] = useState("");
  const [pageConfirmIndex, setPageConfirmIndex] = useState<number | null>(null);

  const loadPageList = useCallback(async () => {
    setPageOpsBusy("读取中…");
    try {
      setPageList(await api.galleryPageList(archiveId));
    } catch (error) {
      onNotify(`读取图片清单失败：${String(error)}`, "error");
    } finally {
      setPageOpsBusy("");
    }
  }, [archiveId, onNotify]);

  useEffect(() => {
    void loadPageList();
  }, [loadPageList]);

  // 重打包进度。
  useEffect(() => {
    const unlisten = listen<{
      archiveId: number;
      stage: string;
      done: number;
      total: number;
    }>("gallery://pageops", (event) => {
      if (event.payload.archiveId !== archiveId) return;
      setPageProgress(`${event.payload.stage} ${event.payload.done}/${event.payload.total}`);
    });
    return () => {
      void unlisten.then((off) => off());
    };
  }, [archiveId]);

  const deletePage = async (index: number) => {
    if (pageConfirmIndex !== index) {
      setPageConfirmIndex(index);
      return;
    }
    setPageConfirmIndex(null);
    setPageOpsBusy("删除中…");
    try {
      const remaining = await api.galleryPageDelete(archiveId, index);
      onNotify(`已删除第 ${index + 1} 页，剩 ${remaining} 页（可在回收站二次删除）`, "success");
      await loadPageList();
      void emit("d2://download", { ok: true });
    } catch (error) {
      onNotify(`删除失败：${String(error)}`, "error");
    } finally {
      setPageOpsBusy("");
    }
  };

  const addPages = async () => {
    setPageOpsBusy("等待选择图片…");
    try {
      const [added, total] = await api.galleryPageAdd(archiveId);
      onNotify(`已追加 ${added} 页，现在共 ${total} 页`, "success");
      await loadPageList();
      void emit("d2://download", { ok: true });
    } catch (error) {
      const message = String(error);
      if (!message.includes("没有选择图片")) {
        onNotify(`追加失败：${message}`, "error");
      }
    } finally {
      setPageOpsBusy("");
    }
  };

  return (
    <div className="page-ops">
      {pageOpsBusy && (
        <p className="settings-hint">
          {pageOpsBusy}
          {pageProgress ? ` · ${pageProgress}` : ""}
        </p>
      )}
      {pageList?.kind !== "zip" && pageList && (
        <p className="settings-hint">
          {pageList.kind === "unsupported"
            ? "7z/RAR 暂不支持单页增删：先解压再重新打包成 ZIP/CBZ。"
            : "读取中…"}
        </p>
      )}
      {pageList?.encrypted && (
        <p className="settings-hint">注意：这是加密包，增删后新包会变成不加密。</p>
      )}
      {pageList?.kind === "zip" && (
        <>
          <button
            type="button"
            className="btn btn-primary btn-compact"
            disabled={Boolean(pageOpsBusy)}
            onClick={() => void addPages()}
          >
            添加图片（追加到末尾）
          </button>
          <div className="page-ops-list">
            {pageList.pages.map((page) => (
              <div className="page-ops-row" key={page.index}>
                <span className="page-ops-index">{page.index + 1}</span>
                <span className="page-ops-name" title={page.name}>
                  {page.name}
                </span>
                <button
                  type="button"
                  className={`btn btn-compact ${
                    pageConfirmIndex === page.index ? "net-dl-danger" : "btn-ghost"
                  }`}
                  disabled={Boolean(pageOpsBusy) || pageList.pages.length <= 1}
                  onClick={() => void deletePage(page.index)}
                  title={
                    pageConfirmIndex === page.index
                      ? "再点一次确认删除（先进回收站）"
                      : "删除这一页"
                  }
                >
                  {pageConfirmIndex === page.index ? "确认？" : "删"}
                </button>
              </div>
            ))}
            {pageList.pages.length === 0 && <p className="settings-hint">没有读到任何图片页。</p>}
          </div>
        </>
      )}
      {!pageList && <ImportIcon width={14} height={14} />}
    </div>
  );
}
