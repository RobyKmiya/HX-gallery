/**
 * 壁纸来源 → 图片 URL。
 *
 * 壁纸锁和新页背景共用同一份来源格式，三种来源各自走已有协议：
 * - file    → asset 协议（convertFileSrc）
 * - loose   → loose://（散图收藏夹里的图）
 * - archive → gallery://（画廊封面或指定页）
 */
import { api, assetUrl, readerPageUrl } from "./api";
import type { WallpaperSource } from "./types";

/** 把一条来源转成可用的图片 URL；不合法返回 null。 */
export function wallpaperUrl(source: WallpaperSource): string | null {
  if (!source?.reference) return null;
  switch (source.kind) {
    case "file":
      return assetUrl(source.reference);
    case "loose": {
      const id = Number(source.reference);
      return Number.isFinite(id) && id > 0 ? api.looseImageUrl("img", id) : null;
    }
    case "archive": {
      const [archiveText, pageText] = source.reference.split(":");
      const archiveId = Number(archiveText);
      if (!Number.isFinite(archiveId) || archiveId <= 0) return null;
      const page = Number(pageText);
      return readerPageUrl(archiveId, Number.isFinite(page) && page >= 0 ? page : 0);
    }
    default:
      return null;
  }
}

/** 设置页里给壁纸起个可读名字。 */
export function describeWallpaper(source: WallpaperSource): string {
  switch (source.kind) {
    case "file":
      return source.reference.split(/[\\/]/).pop() || source.reference;
    case "loose":
      return `散图 #${source.reference}`;
    case "archive": {
      const [archiveId, page] = source.reference.split(":");
      const pageNumber = Number(page);
      return `画廊 #${archiveId} 第 ${(Number.isFinite(pageNumber) ? pageNumber : 0) + 1} 页`;
    }
    default:
      return source.reference;
  }
}
