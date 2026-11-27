/**
 * 壁纸来源 → 图片 URL。
 *
 * 壁纸锁和新页背景共用同一份来源格式，三种来源各自走已有协议：
 * - file    → asset 协议（convertFileSrc）
 * - loose   → loose://（散图收藏夹里的图）
 * - archive → gallery://（画廊封面或指定页）
 */
import { api, assetUrl, readerPageUrl, wallpaperCacheFileUrl } from "./api";
import type { WallpaperSource } from "./types";

/**
 * 壁纸展示地址：优先本地缓存（外接硬盘睡不睡都不影响开屏），没缓存退回
 * 原地址。结果顺序与入参对齐，不合法的项是空串。
 */
export async function wallpaperDisplayUrls(walls: WallpaperSource[]): Promise<string[]> {
  try {
    const names = await api.wallpaperCacheUrls(walls);
    return walls.map((wall, index) => {
      const name = names[index];
      if (name) return wallpaperCacheFileUrl(name);
      return wallpaperUrl(wall) ?? "";
    });
  } catch {
    return walls.map((wall) => wallpaperUrl(wall) ?? "");
  }
}

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

/**
 * 取一张壁纸的「预览用小图」（设置页列表 / 新页背景用）。
 *
 * 不要直接拿 wallpaperUrl 当缩略图：画廊来源走 gallery://page 会让后端解压整页原图
 * （动图几百帧、几十兆），多张一起就卡死。这里画廊走缩略图接口、散图走 thumb，
 * 只有本地文件读原图（用户自己选的）。
 */
export async function wallpaperPreviewUrl(source: WallpaperSource): Promise<string | null> {
  if (source.kind === "archive") {
    const [idText, pageText] = source.reference.split(":");
    const archiveId = Number(idText);
    const page = Number(pageText);
    if (!Number.isFinite(archiveId) || archiveId <= 0) return null;
    try {
      const thumb = await api.readReaderThumbnail(
        archiveId,
        Number.isFinite(page) ? page : 0,
        720,
      );
      return thumb.dataUrl;
    } catch {
      return null;
    }
  }
  if (source.kind === "loose") {
    const id = Number(source.reference);
    return Number.isFinite(id) && id > 0 ? api.looseImageUrl("thumb", id) : null;
  }
  return assetUrl(source.reference);
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
