import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../api";
import { notifyModalOpen, notifyModalClose } from "../modalBus";
import type { NetDlStatus } from "../types";
import { CloseIcon, PlayIcon, PauseIcon, TrashIcon } from "./Icons";

/**
 * 网络画集下载管理器。
 *
 * 管理员 2026-11-26：多链接下载时画匣卡死、操作不了；要能「细看每项、预览内容、
 * 启停各项、并行下载」。后端已改成多任务（每项独立状态/取消/暂停 + 全局闸门），
 * 这里做对应的任务卡片面板：一次轮询拉回所有任务，每项自己有 暂停/继续/取消/移除。
 */
interface NetGalleryDialogProps {
  onClose: () => void;
  onDone?: () => void;
  embedded?: boolean;
}

interface DownloadItem {
  id: string;
  url: string;
  status: NetDlStatus;
  /** 预览元数据（解析后填，下载中也会随状态更新标题/标签）。 */
  previewed: boolean;
}

const EMPTY: NetDlStatus = {
  running: false,
  finished: false,
  error: "",
  stage: "",
  done: 0,
  total: 0,
  title: "",
  tags: [],
  path: "",
};

function shortUrl(url: string): string {
  try {
    const u = new URL(url);
    return u.hostname + (u.pathname.length > 1 ? u.pathname.slice(0, 42) : "");
  } catch {
    return url.length > 46 ? `${url.slice(0, 46)}…` : url;
  }
}

function itemPercent(s: NetDlStatus): number {
  return s.total > 0 ? Math.min(100, Math.round((s.done / s.total) * 100)) : 0;
}

function itemState(s: NetDlStatus): "idle" | "running" | "paused" | "done" | "failed" | "canceled" {
  if (s.stage === "已暂停") return "paused";
  if (s.stage === "已取消") return "canceled";
  if (s.finished) return "done";
  if (s.error) return "failed";
  if (s.running) return "running";
  return "idle";
}

const STATE_LABEL: Record<ReturnType<typeof itemState>, string> = {
  idle: "等待",
  running: "下载中",
  paused: "已暂停",
  done: "已完成",
  failed: "失败",
  canceled: "已取消",
};

export default function NetGalleryDialog({
  onClose,
  onDone,
  embedded = false,
}: NetGalleryDialogProps) {
  const [url, setUrl] = useState("");
  const [items, setItems] = useState<DownloadItem[]>([]);
  const [error, setError] = useState("");
  const [expanded, setExpanded] = useState<string | null>(null);
  const pollRef = useRef<number | null>(null);
  const doneNotified = useRef(false);

  useEffect(() => {
    if (!embedded) notifyModalOpen();
    return () => {
      if (!embedded) notifyModalClose();
      if (pollRef.current !== null) window.clearInterval(pollRef.current);
    };
  }, [embedded]);

  // 一次轮询拉回所有任务进度；这是唯一的定时器，不会再每项各开一个。
  useEffect(() => {
    if (items.length === 0) return;
    let alive = true;
    const tick = async () => {
      try {
        const all = await api.netDlStatusAll();
        if (!alive) return;
        const map = new Map(all);
        setItems((prev) =>
          prev.map((item) => {
            const status = map.get(item.id);
            if (!status) return item;
            return {
              ...item,
              status,
              previewed: item.previewed || Boolean(status.title),
            };
          }),
        );
        // 全部结束且至少完成过一项 → 刷新库一次。
        const allDone = all.length > 0 && all.every(([, s]) => !s.running);
        if (allDone && !doneNotified.current) {
          doneNotified.current = true;
          onDone?.();
        }
        if (!allDone) doneNotified.current = false;
      } catch {
        /* 轮询失败下次再来，不打断 */
      }
    };
    void tick();
    pollRef.current = window.setInterval(() => void tick(), 600);
    return () => {
      alive = false;
      if (pollRef.current !== null) window.clearInterval(pollRef.current);
    };
  }, [items.length, onDone]);

  /** 开始：把每行链接做成一个独立任务，全部提交（后端并行 + 闸门限流）。 */
  const startAll = async () => {
    const targets = url
      .split(/\r?\n+/)
      .map((line) => line.trim())
      .filter(Boolean);
    if (!targets.length) {
      setError("先把作品页链接贴进来");
      return;
    }
    setError("");
    doneNotified.current = false;
    const newItems: DownloadItem[] = targets.map((target) => ({
      id: crypto.randomUUID(),
      url: target,
      status: { ...EMPTY, running: true, stage: "准备中…" },
      previewed: false,
    }));
    setItems((prev) => [...prev, ...newItems]);
    setUrl("");
    for (const item of newItems) {
      try {
        await api.netDlStartTask(item.id, item.url);
      } catch (e) {
        setError(`提交失败：${String(e)}`);
        setItems((prev) =>
          prev.map((it) =>
            it.id === item.id
              ? { ...it, status: { ...EMPTY, error: String(e), stage: "提交失败" } }
              : it,
          ),
        );
      }
    }
  };

  /** 预览单项：解析标题/标签/页数，填进卡片（不下载）。 */
  const previewOne = async (item: DownloadItem) => {
    try {
      const info = await api.netDlPreview(item.url);
      setItems((prev) =>
        prev.map((it) =>
          it.id === item.id
            ? { ...it, status: { ...it.status, ...info, stage: info.stage || "已解析" }, previewed: true }
            : it,
        ),
      );
      setExpanded(item.id);
    } catch (e) {
      setError(`解析失败：${String(e)}`);
    }
  };

  const togglePause = useCallback(async (item: DownloadItem) => {
    const paused = itemState(item.status) !== "paused";
    try {
      await api.netDlPauseTask(item.id, paused);
    } catch (e) {
      setError(`操作失败：${String(e)}`);
    }
  }, []);

  const cancelOne = useCallback(async (id: string) => {
    try {
      await api.netDlCancelTask(id);
    } catch (e) {
      setError(`取消失败：${String(e)}`);
    }
  }, []);

  const removeOne = useCallback(async (id: string) => {
    try {
      await api.netDlRemoveTask(id);
    } catch {
      /* 移除本地项即可 */
    }
    setItems((prev) => prev.filter((it) => it.id !== id));
  }, []);

  /** 单项重试：换一个新任务 id，原地重排（后端不认旧 id 的残留状态）。 */
  const retryOne = useCallback(async (item: DownloadItem) => {
    const nextId = crypto.randomUUID();
    setItems((prev) =>
      prev.map((it) =>
        it.id === item.id
          ? { ...it, id: nextId, status: { ...EMPTY, running: true, stage: "重新排队…" } }
          : it,
      ),
    );
    try {
      await api.netDlStartTask(nextId, item.url);
    } catch (e) {
      setError(`重试失败：${String(e)}`);
    }
  }, []);

  const cancelAll = async () => {
    for (const item of items) {
      if (item.status.running) await cancelOne(item.id);
    }
  };

  const activeCount = items.filter((it) => itemState(it.status) === "running" || itemState(it.status) === "paused").length;
  const failedItems = items.filter((it) => {
    const state = itemState(it.status);
    return state === "failed" || state === "canceled";
  });
  const failedCount = failedItems.length;

  /** 一键重试所有失败/取消项：逐个换新 id 重排，失败的继续留在列表里。 */
  const retryAllFailed = async () => {
    for (const item of failedItems) {
      await retryOne(item);
    }
  };

  return (
    <div
      className={embedded ? "panel-embedded" : "modal-backdrop"}
      onClick={embedded ? undefined : onClose}
    >
      <div
        className={`modal net-dl-dialog${embedded ? " is-embedded" : ""}`}
        style={embedded ? undefined : { width: "min(760px, 94vw)" }}
        onClick={(event) => event.stopPropagation()}
      >
        <header className="modal-title-row">
          <div>
            <h2>网络画集</h2>
            <p className="modal-hint">
              贴作品页链接，解析标题 / 标签 / 页数，下载整本打包入库。多项并行，每项可单独启停。
            </p>
          </div>
          <div className="net-dl-head-right">
            {activeCount > 0 && (
              <span className="status-chip net-dl-active">
                <span className="net-dl-pulse" />
                {activeCount} 项进行中
              </span>
            )}
            {!embedded && (
              <button type="button" className="icon-button" onClick={onClose} aria-label="关闭">
                <CloseIcon />
              </button>
            )}
          </div>
        </header>

        <section className="net-dl-compose">
          <textarea
            value={url}
            onChange={(event) => setUrl(event.target.value)}
            placeholder={
              "一行一个作品页链接，例如：\nhttps://18comic.vip/photo/1492995\nhttps://exhentai.org/g/1234567/abcdef1234/\nhttps://nhentai.net/g/615034/"
            }
            spellCheck={false}
            rows={3}
            className="net-dl-input"
            onKeyDown={(event) => {
              if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) {
                event.preventDefault();
                void startAll();
              }
            }}
          />
          <div className="net-dl-compose-bar">
            <p className="net-dl-tip">
              支持 exhentai / e-hentai / nhentai / 18comic（单章 /photo/）。
              需要登录时，在内置浏览器登录后到「设置 → 浏览器数据 → 登录态」保存站点域名。
            </p>
            <button type="button" className="btn btn-primary" onClick={() => void startAll()}>
              <span className="net-dl-btn-icon">＋</span> 加入下载
            </button>
          </div>
        </section>

        {error ? <p className="net-dl-error">{error}</p> : null}

        <div className="net-dl-list-head">
          <span>下载项</span>
          <span className="net-dl-count">{items.length} 项</span>
        </div>
        <div className="net-dl-list">
          {items.length === 0 ? (
            <div className="empty-state net-dl-empty">
              <div className="net-dl-empty-icon">⬇</div>
              <p>还没有下载项</p>
              <p className="net-dl-empty-sub">把作品页链接贴到上面，一行一个，点「加入下载」</p>
            </div>
          ) : (
            items.map((item) => {
              const state = itemState(item.status);
              const percent = itemPercent(item.status);
              const isOpen = expanded === item.id;
              return (
                <div className={`net-dl-item is-${state}`} key={item.id}>
                  <div className="net-dl-item-main">
                    <button
                      type="button"
                      className="net-dl-item-toggle"
                      onClick={() => setExpanded(isOpen ? null : item.id)}
                      title={isOpen ? "收起" : "细看 / 预览"}
                    >
                      {isOpen ? "▾" : "▸"}
                    </button>
                    <div className="net-dl-item-body">
                      <div className="net-dl-item-line1">
                        <span className="net-dl-item-title">
                          {item.status.title || shortUrl(item.url)}
                        </span>
                        <span className={`net-dl-state net-dl-state-${state}`}>
                          {STATE_LABEL[state]}
                        </span>
                      </div>
                      <div className="net-dl-item-url">{shortUrl(item.url)}</div>
                      <div className="net-dl-progress-row">
                        <div className="progress-track net-dl-track">
                          <div
                            className={`progress-fill net-dl-fill${state === "paused" ? " is-paused" : ""}`}
                            style={{ width: `${state === "done" ? 100 : percent}%` }}
                          />
                        </div>
                        <span className="net-dl-percent">{percent}%</span>
                      </div>
                      <div className="net-dl-item-stage">
                        {item.status.stage}
                        {item.status.total > 0
                          ? ` · ${item.status.done}/${item.status.total} 页`
                          : ""}
                      </div>
                    </div>
                    <div className="net-dl-item-actions">
                      {!item.previewed && state !== "running" && (
                        <button type="button" className="btn btn-quiet btn-compact" onClick={() => void previewOne(item)}>
                          预览
                        </button>
                      )}
                      {state === "running" || state === "paused" ? (
                        <>
                          <button
                            type="button"
                            className="btn btn-quiet btn-compact"
                            onClick={() => void togglePause(item)}
                            title={state === "paused" ? "继续" : "暂停"}
                          >
                            {state === "paused" ? <PlayIcon /> : <PauseIcon />}
                          </button>
                          <button
                            type="button"
                            className="btn btn-quiet btn-compact net-dl-danger"
                            onClick={() => void cancelOne(item.id)}
                            title="取消"
                          >
                            取消
                          </button>
                        </>
                      ) : (
                        <>
                          {(state === "failed" || state === "canceled") && (
                            <button
                              type="button"
                              className="btn btn-quiet btn-compact"
                              onClick={() => void retryOne(item)}
                              title="重试这一项"
                            >
                              重试
                            </button>
                          )}
                          <button
                            type="button"
                            className="btn btn-quiet btn-compact net-dl-danger"
                            onClick={() => void removeOne(item.id)}
                            title="移除"
                          >
                            <TrashIcon />
                          </button>
                        </>
                      )}
                    </div>
                  </div>

                  {isOpen ? (
                    <div className="net-dl-item-detail">
                      <div className="net-dl-detail-url">{item.url}</div>
                      {item.status.tags.length > 0 ? (
                        <div className="tag-list net-dl-tags">
                          {item.status.tags.slice(0, 60).map(([namespace, value]) => (
                            <span className="tag-chip" key={`${namespace}:${value}`}>
                              {namespace}: {value}
                            </span>
                          ))}
                        </div>
                      ) : (
                        <p className="net-dl-detail-hint">还没有标签，点「预览」解析作品页。</p>
                      )}
                      {item.status.error ? (
                        <p className="net-dl-error">{item.status.error}</p>
                      ) : null}
                      {item.status.path && item.status.path !== "已取消" ? (
                        <p className="net-dl-detail-hint">已入库：{item.status.path}</p>
                      ) : null}
                    </div>
                  ) : null}
                </div>
              );
            })
          )}
        </div>

        <footer className="modal-actions net-dl-footer">
          <button type="button" className="btn" onClick={onClose}>
            关闭
          </button>
          {activeCount > 0 ? (
            <button type="button" className="btn" onClick={() => void cancelAll()}>
              全部取消
            </button>
          ) : null}
          {failedCount > 0 ? (
            <button type="button" className="btn" onClick={() => void retryAllFailed()}>
              重试失败项（{failedCount}）
            </button>
          ) : null}
          <span className="net-dl-footer-hint">
            Ctrl + Enter 快速加入 · 下载在后台进行，关掉本页也会继续
          </span>
        </footer>
      </div>
    </div>
  );
}
