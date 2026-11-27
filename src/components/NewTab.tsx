import { useEffect, useMemo, useState } from "react";
import { api } from "../api";
import type { Collection, NavIntent, WallpaperLockConfig } from "../types";
import { wallpaperPreviewUrl } from "../wallpapers";
import {
  createBookmark,
  readBookmarks,
  saveBookmarks,
  type Bookmark,
} from "../bookmarks";

/**
 * 浏览器「新页」。
 *
 * 刻意做成 React 组件而不是子 webview 里的静态页：新页要能导航到画匣任意内容，
 * 而子 webview 拿不到主窗 IPC；放在 WorkspaceTabs 里就天然能切标签 / 带筛选。
 * 背景复用壁纸锁配置（多张轮换），统一走预览小图，避免解压原图卡顿。
 *
 * 顶部自带一条工具栏（38px，与 webview 内 D2 工具栏同高），新页/网页切换时视觉连续。
 */
interface NewTabProps {
  libraryName?: string;
  onNavigate: (intent: NavIntent) => void;
  onOpenTab: (kind: string) => void;
  onBrowse: (url: string) => void;
}

/** 把输入归一成可访问 URL：全 URL 原样、看起来像域名补 https、其余走 Bing 搜索。 */
function normalizeAddress(value: string): string | null {
  const trimmed = value.trim();
  if (!trimmed) return null;
  if (/^https?:\/\//i.test(trimmed)) return trimmed;
  if (trimmed.includes(".") && !trimmed.includes(" ")) return `https://${trimmed}`;
  return `https://www.bing.com/search?q=${encodeURIComponent(trimmed)}`;
}

function NewTabToolbar({ onBrowse }: { onBrowse: (url: string) => void }) {
  const [address, setAddress] = useState("");

  const submit = () => {
    const url = normalizeAddress(address);
    if (!url) return;
    onBrowse(url);
    setAddress("");
  };

  return (
    <div className="new-tab-toolbar">
      {/* 新页没有历史，不放灰着的 ← → ⟳（看着像坏了）；进网页后由内置工具栏接管。 */}
      <button type="button" className="is-active" title="画匣新页">
        ⌂
      </button>
      <form
        className="new-tab-toolbar-addr"
        onSubmit={(event) => {
          event.preventDefault();
          submit();
        }}
      >
        <input
          value={address}
          onChange={(event) => setAddress(event.target.value)}
          placeholder="输入网址，或搜索后回车"
          spellCheck={false}
        />
      </form>
    </div>
  );
}

export default function NewTab({ libraryName, onNavigate, onOpenTab, onBrowse }: NewTabProps) {
  const [collections, setCollections] = useState<Collection[]>([]);
  const [lock, setLock] = useState<WallpaperLockConfig | null>(null);
  const [urls, setUrls] = useState<string[]>([]);
  const [index, setIndex] = useState(0);

  const [bookmarks, setBookmarks] = useState<Bookmark[]>(() => readBookmarks());
  const [adding, setAdding] = useState(false);
  const [draftName, setDraftName] = useState("");
  const [draftUrl, setDraftUrl] = useState("");

  useEffect(() => {
    void api.listCollections().then(setCollections).catch(() => {});
  }, []);

  // 背景：读壁纸锁配置，逐张取预览小图（画廊走缩略图，不会卡）。
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      const config = await api.getWallpaperLock().catch(() => null);
      if (!config || cancelled) return;
      const list = (
        await Promise.all(config.walls.map((wall) => wallpaperPreviewUrl(wall).catch(() => null)))
      ).filter((url): url is string => Boolean(url));
      if (cancelled) return;
      setLock(config);
      setUrls(list);
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (urls.length <= 1 || !lock || lock.intervalSecs <= 0) return;
    const timer = window.setInterval(
      () => setIndex((current) => (current + 1) % urls.length),
      lock.intervalSecs * 1000,
    );
    return () => window.clearInterval(timer);
  }, [urls.length, lock]);

  const current = useMemo(
    () => (urls.length > 0 ? urls[index % urls.length] : null),
    [urls, index],
  );

  const addBookmark = () => {
    const url = normalizeAddress(draftUrl);
    if (!url) return;
    const next = [...bookmarks, createBookmark(draftName, url)];
    setBookmarks(next);
    saveBookmarks(next);
    setDraftName("");
    setDraftUrl("");
    setAdding(false);
  };

  const removeBookmark = (id: string) => {
    const next = bookmarks.filter((item) => item.id !== id);
    setBookmarks(next);
    saveBookmarks(next);
  };

  return (
    <div className="new-tab-wrap">
      <NewTabToolbar onBrowse={onBrowse} />
      <div className="new-tab">
        {/* 壁纸单独一层、不跟着滚动：以前 background-image 挂在这个 overflow 容器上，
            滚动时每帧都要重绘整张放大图，新页滑动明显掉帧（管理员 2026-11-27）。 */}
        <div
          className="new-tab-bg"
          aria-hidden="true"
          style={current ? { backgroundImage: `url("${current}")` } : undefined}
        />
        <div className="new-tab-shade" aria-hidden="true" />
        <div className="new-tab-scroll">
        <div className="new-tab-body">
          <p className="new-tab-brand">{libraryName ? `画匣 · ${libraryName}` : "画匣"}</p>

          <div className="new-tab-grid">
            <button type="button" onClick={() => onNavigate({ kind: "home" })}>
              <span className="new-tab-glyph">🏠</span>
              <span>全部画集</span>
            </button>
            <button type="button" onClick={() => onNavigate({ kind: "recent" })}>
              <span className="new-tab-glyph">🕘</span>
              <span>最近阅读</span>
            </button>
            <button type="button" onClick={() => onNavigate({ kind: "loose" })}>
              <span className="new-tab-glyph">🖼</span>
              <span>散图收藏夹</span>
            </button>
            <button type="button" onClick={() => onOpenTab("tags")}>
              <span className="new-tab-glyph">#</span>
              <span>标签中心</span>
            </button>
            <button type="button" onClick={() => onOpenTab("net")}>
              <span className="new-tab-glyph">↓</span>
              <span>链接下载</span>
            </button>
          </div>

          {collections.length > 0 && (
            <div className="new-tab-collections">
              {collections.slice(0, 8).map((collection) => (
                <button
                  key={collection.id}
                  type="button"
                  onClick={() => onNavigate({ kind: "collection", id: collection.id })}
                  title={collection.name}
                >
                  {collection.name}
                </button>
              ))}
            </div>
          )}

          {bookmarks.length > 0 && (
            <div className="new-tab-bookmarks">
              {bookmarks.map((bookmark) => (
                <div className="new-tab-bookmark" key={bookmark.id}>
                  <button
                    type="button"
                    className="new-tab-bookmark-main"
                    onClick={() => onBrowse(bookmark.url)}
                    title={bookmark.url}
                  >
                    <span className="new-tab-bookmark-glyph">
                      {bookmark.name.slice(0, 1).toUpperCase()}
                    </span>
                    <span className="new-tab-bookmark-name">{bookmark.name}</span>
                  </button>
                  <button
                    type="button"
                    className="new-tab-bookmark-remove"
                    onClick={() => removeBookmark(bookmark.id)}
                    title="删除书签"
                  >
                    ×
                  </button>
                </div>
              ))}
            </div>
          )}

          <div className="new-tab-add">
            {adding ? (
              <>
                <input
                  value={draftName}
                  onChange={(event) => setDraftName(event.target.value)}
                  placeholder="名称（可空）"
                />
                <input
                  value={draftUrl}
                  onChange={(event) => setDraftUrl(event.target.value)}
                  placeholder="网址，如 e-hentai.org"
                  onKeyDown={(event) => {
                    if (event.key === "Enter") addBookmark();
                  }}
                />
                <button type="button" onClick={addBookmark}>
                  保存
                </button>
                <button
                  type="button"
                  onClick={() => {
                    setAdding(false);
                    setDraftName("");
                    setDraftUrl("");
                  }}
                >
                  取消
                </button>
              </>
            ) : (
              <button type="button" className="new-tab-add-btn" onClick={() => setAdding(true)}>
                ＋ 添加网站书签
              </button>
            )}
          </div>
        </div>
        </div>
      </div>
    </div>
  );
}
