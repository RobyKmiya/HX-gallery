import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import type {
  ArchivePage,
  ArchiveQuery,
  BulkTagReport,
  Collection,
  ImportOptions,
  ImportReport,
  LibraryState,
  GmScriptMeta,
  LooseImage,
  LooseImageCollection,
  Queue,
  QueueFillOptions,
  QueueFillReport,
  AnimMeta,
  ReaderBookmark,
  ReaderInfo,
  ReaderPage,
  ReaderThumb,
  ReleaseState,
  ScanReport,
  SeriesSummary,
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

  /**
   * 打开 / 导航浏览窗口，**不抓取**。
   *
   * 没有域名白名单：站点地址灵活多变，硬编码只会把人挡在登录页外面。
   * 能不能抓由解析器决定，和能不能浏览是两件事。
   */
  d2Open: (url: string) => invoke<void>("d2_open", { url }),

  /** 浏览窗口：后退 / 前进 / 刷新 / 关闭。 */
  d2Back: () => invoke<void>("d2_back"),
  d2Forward: () => invoke<void>("d2_forward"),
  d2Reload: () => invoke<void>("d2_reload"),
  d2CloseWindow: () => invoke<void>("d2_close_window"),

  /** 读当前浏览窗口地址，供「抓取当前页」一键使用（没开窗口返回空串）。 */
  d2CurrentUrl: () => invoke<string>("d2_current_url"),

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

  /** 动画元数据（帧数 / 总时长 / 原始尺寸）。动图由 webview 原生播放，播放模式据此决定停留时长。 */
  readAnimMeta: (archiveId: number, index: number) =>
    invoke<AnimMeta>("read_anim_meta", { archiveId, index }),

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

  // ── 匣浏览器 · 油猴脚本（Chrome/Edge 生态 .user.js）──

  // ── 散图收藏夹（单图多区，只记录引用不迁移）──

  looseListCollections: () => invoke<LooseImageCollection[]>("loose_list_collections"),
  looseCreateCollection: (name: string, folder?: string) =>
    invoke<LooseImageCollection>("loose_create_collection", { name, folder }),
  looseDeleteCollection: (collectionId: number) =>
    invoke<void>("loose_delete_collection", { collectionId }),
  looseScanCollection: (collectionId: number) =>
    invoke<[number, number, number]>("loose_scan_collection", { collectionId }),
  /** 拖放进来的图片：先复制进该区文件夹，再登记。返回 [新增, 缺失, 现有]。 */
  looseImportFiles: (collectionId: number, paths: string[]) =>
    invoke<[number, number, number]>("loose_import_files", { collectionId, paths }),
  looseListImages: (collectionId: number) =>
    invoke<LooseImage[]>("loose_list_images", { collectionId }),
  looseSetRating: (id: number, rating: number) =>
    invoke<void>("loose_set_rating", { id, rating }),
  looseSetFavorite: (id: number, favorite: boolean) =>
    invoke<void>("loose_set_favorite", { id, favorite }),
  looseRemoveImage: (id: number, deleteFile?: boolean) =>
    invoke<void>("loose_remove_image", { id, deleteFile }),
  looseCollectPage: (collectionId: number, archiveId: number, pageIndex: number) =>
    invoke<LooseImage>("loose_collect_page", { collectionId, archiveId, pageIndex }),
  looseExtractPage: (collectionId: number, archiveId: number, pageIndex: number) =>
    invoke<LooseImage>("loose_extract_page", { collectionId, archiveId, pageIndex }),
  looseOpenFolder: (folder: string) => invoke<void>("loose_open_folder", { folder }),

  /** 散图 URL（loose:// 协议：Windows 映射 http://loose.localhost）。 */
  looseImageUrl: (kind: "img" | "thumb", id: number) =>
    LOOSE_ORIGIN +
    `/${kind}/${id}`,

  listGmScripts: () => invoke<GmScriptMeta[]>("gm_list_scripts"),

  /** 从本地 .user.js 文件导入。 */
  addGmScriptFromPath: (path: string) =>
    invoke<GmScriptMeta>("gm_add_script_from_path", { path }),

  /** 从网址拉取 .user.js 源码导入（curl 直连，走系统代理）。 */
  addGmScriptFromUrl: (url: string) =>
    invoke<GmScriptMeta>("gm_add_script_from_url", { url }),

  removeGmScript: (id: string) => invoke<void>("gm_remove_script", { id }),

  setGmScriptEnabled: (id: string, enabled: boolean) =>
    invoke<void>("gm_set_script_enabled", { id, enabled }),

  /** 新建一个空的待看目录（先建壳，再用多选挑入）。 */
  createQueue: (name: string) => invoke<Queue>("create_queue", { name }),

  /** 批量按传入顺序把画集追加进待看目录；返回真正新增的本数。 */
  addManyToQueue: (queueId: number, archiveIds: number[]) =>
    invoke<number>("add_many_to_queue", { queueId, archiveIds }),

  /** 队列内把一本往前/往后挪一格（direction：-1 上移，1 下移）。 */
  moveQueueArchive: (queueId: number, archiveId: number, direction: -1 | 1) =>
    invoke<void>("move_queue_archive", { queueId, archiveId, direction }),

  /** 标签权重：0 表示不想看到带此标签的画集；默认 100。 */
  setTagWeight: (tagId: number, weight: number) =>
    invoke<Tag>("set_tag_weight", { tagId, weight }),

  getReleaseState: () => invoke<ReleaseState>("get_release_state"),

  /** 沉浸阅读：连系统标题栏一起收掉（false = 无边框），退出阅读时记得还原。 */
  setWindowDecorations: (decorated: boolean) =>
    invoke<void>("set_window_decorations", { decorated }),

  /** 老板键第一步：把主窗藏起来。呼回用全局热键 Ctrl+Shift+H。 */
  hideMainWindow: () => invoke<void>("hide_main_window"),

  /** 当前阅读页解码后写入系统剪贴板（动图取第一帧）。 */
  copyReaderPage: (archiveId: number, pageIndex: number) =>
    invoke<void>("copy_reader_page", { archiveId, pageIndex }),

  /** 更新收藏夹侧栏图标（形状 + 填充色）。 */
  updateCollectionStyle: (collectionId: number, icon: string, color: string) =>
    invoke<void>("update_collection_style", { collectionId, icon, color }),

  /** 在资源管理器中定位并选中画集文件。 */
  revealInExplorer: (path: string) =>
    invoke<void>("reveal_in_explorer", { path }),

  /** 当前阅读页导出 PNG 并设为桌面壁纸。 */
  setDesktopWallpaper: (archiveId: number, pageIndex: number) =>
    invoke<void>("set_desktop_wallpaper", { archiveId, pageIndex }),

  // ---------- 合集（连载盒子） ----------
  listSeries: () => invoke<SeriesSummary[]>("list_series"),
  createSeries: (name: string, archiveIds: number[]) =>
    invoke<number>("create_series", { name, archiveIds }),
  updateSeries: (seriesId: number, name: string, note: string | null) =>
    invoke<void>("update_series", { seriesId, name, note }),
  dissolveSeries: (seriesId: number) =>
    invoke<void>("dissolve_series", { seriesId }),
  seriesSetCover: (seriesId: number, archiveId: number | null) =>
    invoke<void>("series_set_cover", { seriesId, archiveId }),
  seriesReorder: (seriesId: number, archiveIds: number[]) =>
    invoke<void>("series_reorder", { seriesId, archiveIds }),
  seriesSetLabel: (seriesId: number, archiveId: number, label: string | null) =>
    invoke<void>("series_set_label", { seriesId, archiveId, label }),
  seriesAddArchives: (seriesId: number, archiveIds: number[]) =>
    invoke<number>("series_add_archives", { seriesId, archiveIds }),
  seriesRemoveArchive: (seriesId: number, archiveId: number) =>
    invoke<void>("series_remove_archive", { seriesId, archiveId }),

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

/** 散图自定义协议（loose://）在 Windows 上同样映射成 http 主机。 */
const LOOSE_ORIGIN =
  typeof navigator !== "undefined" && navigator.userAgent.includes("Windows")
    ? "http://loose.localhost"
    : "loose://localhost";

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
