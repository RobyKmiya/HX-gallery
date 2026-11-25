import { useEffect, useMemo, useState } from "react";
import { api } from "../api";
import type { Collection, NavIntent, WallpaperLockConfig } from "../types";
import { wallpaperPreviewUrl } from "../wallpapers";

/**
 * 浏览器「新页」。
 *
 * 刻意做成 React 组件而不是子 webview 里的静态页：新页要能导航到画匣任意内容，
 * 而子 webview 拿不到主窗 IPC；放在 WorkspaceTabs 里就天然能切标签 / 带筛选。
 * 背景复用壁纸锁的配置（多张轮换），统一走预览小图，避免解压原图卡顿。
 */
interface NewTabProps {
  libraryName?: string;
  onNavigate: (intent: NavIntent) => void;
  onOpenTab: (kind: string) => void;
  onBrowse: (url: string) => void;
}

export default function NewTab({ libraryName, onNavigate, onOpenTab, onBrowse }: NewTabProps) {
  const [collections, setCollections] = useState<Collection[]>([]);
  const [lock, setLock] = useState<WallpaperLockConfig | null>(null);
  const [urls, setUrls] = useState<string[]>([]);
  const [index, setIndex] = useState(0);
  const [address, setAddress] = useState("");

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

  const openAddress = () => {
    const value = address.trim();
    if (!value) return;
    const url = /^https?:\/\//i.test(value)
      ? value
      : value.includes(".") && !value.includes(" ")
        ? `https://${value}`
        : `https://www.bing.com/search?q=${encodeURIComponent(value)}`;
    onBrowse(url);
    setAddress("");
  };

  return (
    <div
      className="new-tab"
      style={current ? { backgroundImage: `url("${current}")` } : undefined}
    >
      <div className="new-tab-shade" aria-hidden="true" />
      <div className="new-tab-body">
        <p className="new-tab-brand">{libraryName ? `画匣 · ${libraryName}` : "画匣"}</p>

        <form
          className="new-tab-address"
          onSubmit={(event) => {
            event.preventDefault();
            openAddress();
          }}
        >
          <input
            value={address}
            onChange={(event) => setAddress(event.target.value)}
            placeholder="输入网址，或搜索后回车"
            spellCheck={false}
          />
          <button type="submit">打开</button>
        </form>

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
            <span>网络画集</span>
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
      </div>
    </div>
  );
}
