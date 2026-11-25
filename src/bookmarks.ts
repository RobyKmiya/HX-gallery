/**
 * 浏览器新页的网站收藏夹（书签）。
 *
 * 纯前端 localStorage：书签是轻量个人数据，不绑库、不进 DB，换库也还在。
 * 格式简单，坏数据一律丢弃而不是崩掉新页。
 */
export interface Bookmark {
  id: string;
  name: string;
  url: string;
}

const KEY = "huaxia.bookmarks";

function isBookmark(value: unknown): value is Bookmark {
  if (!value || typeof value !== "object") return false;
  const item = value as Record<string, unknown>;
  return (
    typeof item.id === "string" &&
    typeof item.name === "string" &&
    typeof item.url === "string" &&
    item.url.length > 0
  );
}

export function readBookmarks(): Bookmark[] {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.filter(isBookmark).slice(0, 60);
  } catch {
    return [];
  }
}

export function saveBookmarks(list: Bookmark[]): void {
  try {
    localStorage.setItem(KEY, JSON.stringify(list.slice(0, 60)));
  } catch {
    // 存储满/被禁就静默放弃，不影响新页使用。
  }
}

/** 域名转成可读名，用于「添加书签」时的默认名字。 */
export function defaultBookmarkName(url: string): string {
  try {
    const host = new URL(url).hostname.replace(/^www\./, "");
    return host || url;
  } catch {
    return url;
  }
}

export function createBookmark(name: string, url: string): Bookmark {
  return {
    id: `bm-${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`,
    name: name.trim() || defaultBookmarkName(url),
    url: url.trim(),
  };
}
