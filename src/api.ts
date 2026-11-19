import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import type {
  ArchivePage,
  ArchiveQuery,
  BulkTagReport,
  Collection,
  ImportOptions,
  ImportReport,
  LibraryState,
  Queue,
  QueueFillOptions,
  QueueFillReport,
  ReaderBookmark,
  ReaderInfo,
  ReaderPage,
  ReaderThumb,
  ReleaseState,
  ScanReport,
  StageReport,
  Tag,
  TagCategory,
  TagDraft,
  TagGraph,
  TagTranslationInput,
  TrashedArchive,
} from "./types";

export const api = {
  getLibraryState: () => invoke<LibraryState>("get_library_state"),

  openLibrary: (path: string) =>
    invoke<LibraryState>("open_library", { path }),

  queryArchives: (query: ArchiveQuery) =>
    invoke<ArchivePage>("query_archives", { query }),

  importPaths: (paths: string[], options: ImportOptions) =>
    invoke<ImportReport>("import_paths", { paths, options }),

  /** D2 M1：启动一次抓取。立即返回，进度与结果走 `d2://status` / `d2://done` 事件。 */
  d2Start: (url: string) => invoke<void>("d2_start", { url }),

  /** 取消当前抓取并关掉浏览窗口。 */
  d2Cancel: () => invoke<void>("d2_cancel"),

  /** 导入前的暂存：展开路径、给出大小 / 页数 / 疑似重复，不做任何复制。 */
  stageImportPaths: (paths: string[]) =>
    invoke<StageReport>("stage_import_paths", { paths }),

  scanLibrary: () => invoke<ScanReport>("scan_library"),

  setRating: (id: number, rating: number) =>
    invoke<number>("set_rating", { id, rating }),

  setNote: (id: number, note: string | null) =>
    invoke<string | null>("set_note", { id, note }),

  deleteArchive: (archiveId: number) =>
    invoke<void>("delete_archive", { archiveId }),

  restoreArchive: (archiveId: number) =>
    invoke<void>("restore_archive", { archiveId }),

  purgeArchive: (archiveId: number) =>
    invoke<void>("purge_archive", { archiveId }),

  listDeletedArchives: () =>
    invoke<TrashedArchive[]>("list_deleted_archives"),

  listTags: () => invoke<Tag[]>("list_tags"),

  listDeletedTags: () => invoke<Tag[]>("list_deleted_tags"),

  listArchiveTags: (archiveId: number) =>
    invoke<Tag[]>("list_archive_tags", { archiveId }),

  addTag: (archiveId: number, name: string) =>
    invoke<Tag>("add_tag", { archiveId, name }),

  addTagsBulk: (archiveId: number, tags: TagDraft[]) =>
    invoke<BulkTagReport>("add_tags_bulk", { archiveId, tags }),

  removeTag: (archiveId: number, tagId: number) =>
    invoke<void>("remove_tag", { archiveId, tagId }),

  updateTag: (tagId: number, name: string, translatedName: string | null) =>
    invoke<Tag>("update_tag", { tagId, name, translatedName }),

  setTagTranslations: (updates: TagTranslationInput[]) =>
    invoke<number>("set_tag_translations", { updates }),

  mergeTags: (sourceIds: number[], targetId: number) =>
    invoke<Tag>("merge_tags", { sourceIds, targetId }),

  deleteTag: (tagId: number) => invoke<void>("delete_tag", { tagId }),

  restoreTag: (tagId: number) => invoke<Tag>("restore_tag", { tagId }),

  purgeTag: (tagId: number) => invoke<void>("purge_tag", { tagId }),

  listTagCategories: () => invoke<TagCategory[]>("list_tag_categories"),

  createTagCategory: (parentPath: string | null, name: string) =>
    invoke<TagCategory>("create_tag_category", { parentPath, name }),

  renameTagCategory: (path: string, newName: string) =>
    invoke<TagCategory[]>("rename_tag_category", { path, newName }),

  moveTagCategory: (path: string, newParentPath: string | null) =>
    invoke<TagCategory[]>("move_tag_category", { path, newParentPath }),

  deleteTagCategory: (path: string) =>
    invoke<TagCategory[]>("delete_tag_category", { path }),

  setTagCategory: (tagId: number, category: string | null) =>
    invoke<Tag>("set_tag_category", { tagId, category }),

  tagGraph: (minWeight?: number, limit?: number) =>
    invoke<TagGraph>("tag_graph", { minWeight, limit }),

  listCollections: () => invoke<Collection[]>("list_collections"),

  createCollection: (name: string) =>
    invoke<Collection>("create_collection", { name }),

  renameCollection: (collectionId: number, name: string) =>
    invoke<Collection>("rename_collection", { collectionId, name }),

  deleteCollection: (collectionId: number) =>
    invoke<void>("delete_collection", { collectionId }),

  listDeletedCollections: () =>
    invoke<Collection[]>("list_deleted_collections"),

  restoreCollection: (collectionId: number) =>
    invoke<Collection>("restore_collection", { collectionId }),

  purgeCollection: (collectionId: number) =>
    invoke<void>("purge_collection", { collectionId }),

  reorderCollections: (orderedIds: number[]) =>
    invoke<void>("reorder_collections", { orderedIds }),

  setCollectionCover: (collectionId: number, archiveId: number | null) =>
    invoke<Collection>("set_collection_cover", { collectionId, archiveId }),

  addArchivesToCollection: (collectionId: number, archiveIds: number[]) =>
    invoke<number>("add_archives_to_collection", { collectionId, archiveIds }),

  listArchiveCollections: (archiveId: number) =>
    invoke<number[]>("list_archive_collections", { archiveId }),

  setArchiveCollection: (
    archiveId: number,
    collectionId: number,
    member: boolean,
  ) =>
    invoke<void>("set_archive_collection", {
      archiveId,
      collectionId,
      member,
    }),

  openReader: (archiveId: number) =>
    invoke<ReaderInfo>("open_reader", { archiveId }),

  /**
   * 预热一页：后端解压/解码后放进缓存，图片由 readerPageUrl 提供。
   * `current` 表示这页正在显示，后端会把它钉在缓存里，不让预取挤掉。
   */
  readReaderPage: (archiveId: number, index: number, current = false) =>
    invoke<ReaderPage>("read_reader_page", { archiveId, index, current }),

  readReaderThumbnail: (archiveId: number, index: number, maxEdge?: number) =>
    invoke<ReaderThumb>("read_reader_thumbnail", {
      archiveId,
      index,
      maxEdge: maxEdge ?? null,
    }),

  setReadingProgress: (archiveId: number, page: number) =>
    invoke<void>("set_reading_progress", { archiveId, page }),

  addReaderBookmark: (archiveId: number, page: number) =>
    invoke<ReaderBookmark>("add_reader_bookmark", { archiveId, page }),

  removeReaderBookmark: (archiveId: number, bookmarkId: number) =>
    invoke<void>("remove_reader_bookmark", { archiveId, bookmarkId }),

  listQueues: () => invoke<Queue[]>("list_queues"),

  /** 生成（或续满）一个待看目录；同名目录会在此基础上追加。 */
  fillQueue: (name: string, options: QueueFillOptions) =>
    invoke<QueueFillReport>("fill_queue", { name, options }),

  deleteQueue: (queueId: number) => invoke<void>("delete_queue", { queueId }),

  listQueueArchives: (queueId: number) =>
    invoke<number[]>("list_queue_archives", { queueId }),

  /** 手动把单本画集加入待看目录；重复加入返回 false。 */
  addToQueue: (queueId: number, archiveId: number) =>
    invoke<boolean>("add_to_queue", { queueId, archiveId }),

  removeFromQueue: (queueId: number, archiveId: number) =>
    invoke<void>("remove_from_queue", { queueId, archiveId }),

  /** 标签权重：0 表示不想看到带此标签的画集；默认 100。 */
  setTagWeight: (tagId: number, weight: number) =>
    invoke<Tag>("set_tag_weight", { tagId, weight }),

  getReleaseState: () => invoke<ReleaseState>("get_release_state"),

  saveReleaseSettings: (
    sourceDir: string | null,
    autoOnExit: boolean,
    relaunchAfter: boolean,
  ) =>
    invoke<ReleaseState>("save_release_settings", {
      sourceDir,
      autoOnExit,
      relaunchAfter,
    }),

  /** 排好一次发布，但还不退出应用。 */
  scheduleRelease: () =>
    invoke<{ scriptPath: string; logPath: string }>("schedule_release"),

  /** 启动更新脚本并退出应用。 */
  quitForRelease: () => invoke<void>("quit_for_release"),

  cancelRelease: () => invoke<void>("cancel_release"),
};

export function assetUrl(path: string): string {
  return convertFileSrc(path);
}

/**
 * `gallery://` 自定义协议的 URL 前缀。
 *
 * Windows / Android 的 webview 只认 http(s)，Tauri 把自定义协议映射成
 * `http://<scheme>.localhost`；其余平台用 `<scheme>://localhost`。
 */
const GALLERY_ORIGIN =
  typeof navigator !== "undefined" && navigator.userAgent.includes("Windows")
    ? "http://gallery.localhost"
    : "gallery://localhost";

/**
 * 阅读器页面 URL。
 *
 * `token` 只用于重试时强制 webview 重新请求（换 URL 才能摆脱上一次失败的
 * 图片状态），正常翻页始终是 0。
 */
export function readerPageUrl(
  archiveId: number,
  index: number,
  token = 0,
  version?: string,
): string {
  const params = new URLSearchParams();
  // `v` 是压缩包版本号：带它的 URL 与内容一一对应，webview 可以直接缓存，
  // 于是重试 / 切页回来 / 重新挂 <img> 都不必再把几十兆的字节搬一遍。
  if (version) params.set("v", version);
  if (token) params.set("t", String(token));
  const query = params.toString();
  return `${GALLERY_ORIGIN}/page/${archiveId}/${index}${query ? `?${query}` : ""}`;
}
