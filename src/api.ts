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
  WallpaperLockConfig,
  WallpaperSource,
  HistoryEntry,
  CredentialEntry,
  SiteLoginEntry,
  NetDlStatus,
  BlockedEntry,
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
  d2Open: (url: string, adblock?: boolean, home?: string) =>
    invoke<void>("d2_open", { url, adblock: adblock ?? null, home: home ?? null }),

  /** 浏览窗口：后退 / 前进 / 刷新 / 关闭。 */
  d2Back: () => invoke<void>("d2_back"),
  d2Forward: () => invoke<void>("d2_forward"),
  d2Reload: () => invoke<void>("d2_reload"),
  /** 用浏览器里复制的 Cookie 串登录内建浏览器（exhentai 用）。 */
  d2SetCookies: (cookieHeader: string, domains?: string[]) =>
    invoke<number>("d2_set_cookies", { cookieHeader, domains: domains ?? null }),
  /** 一键抓取当前浏览标签页里的全部 tag（返回每行一个 ns: name）。 */
  d2CollectTags: () => invoke<string>("d2_collect_tags"),
  d2CloseWindow: () => invoke<void>("d2_close_window"),

  /** 读当前浏览窗口地址，供「抓取当前页」一键使用（没开窗口返回空串）。 */
  d2CurrentUrl: () => invoke<string>("d2_current_url"),

  /** 网络画集：解析作品页（标题 / 标签 / 页数），不下载。 */
  netDlPreview: (url: string) => invoke<NetDlStatus>("net_dl_preview", { url }),
  /** 网络画集：开始下载整本（旧单任务入口，保留兼容）。 */
  netDlStart: (url: string) => invoke<void>("net_dl_start", { url }),
  netDlStatus: () => invoke<NetDlStatus>("net_dl_status"),
  netDlCancel: () => invoke<void>("net_dl_cancel"),

  // ---------- 网络画集多任务管理器（2026-11-26：并行 + 单项启停） ----------
  /** 以指定 id 启动一项下载（id 由前端 crypto.randomUUID() 生成）。 */
  netDlStartTask: (taskId: string, url: string) =>
    invoke<void>("net_dl_start_task", { taskId, url }),
  /** 一次拉回所有任务的进度快照。 */
  netDlStatusAll: () => invoke<[string, NetDlStatus][]>("net_dl_status_all"),
  /** 只取消某一项（立即 kill 在途 curl）。 */
  netDlCancelTask: (taskId: string) => invoke<void>("net_dl_cancel_task", { taskId }),
  /** 暂停 / 继续某一项。 */
  netDlPauseTask: (taskId: string, paused: boolean) =>
    invoke<void>("net_dl_pause_task", { taskId, paused }),
  /** 从列表移除某一项（会先取消）。 */
  netDlRemoveTask: (taskId: string) => invoke<void>("net_dl_remove_task", { taskId }),

  /** 打开独立的下载面板（范围 / 并发 / 打包都在那儿定）。 */
  dlOpenPanel: () => invoke<void>("dl_open_panel"),

  /** 导入前的暂存：展开路径、给出大小 / 页数 / 疑似重复，不做任何复制。 */
  stageImportPaths: (paths: string[]) =>
    invoke<StageReport>("stage_import_paths", { paths }),

  scanLibrary: () => invoke<ScanReport>("scan_library"),

  setRating: (id: number, rating: number) =>
    invoke<number>("set_rating", { id, rating }),

  setNote: (id: number, note: string | null) =>
    invoke<string | null>("set_note", { id, note }),

  /** 图片管理：zip/cbz 的单页清单。 */
  galleryPageList: (id: number) =>
    invoke<{
      kind: "zip" | "unsupported";
      encrypted: boolean;
      pages: { index: number; name: string; size: number }[];
      pageCount: number;
    }>("gallery_page_list", { id }),
  /** 删除第 index 页（0 基）：重打包，被删页进回收站。返回剩余页数。 */
  galleryPageDelete: (id: number, index: number) =>
    invoke<number>("gallery_page_delete", { id, index }),
  /** 追加图片：原生选择器挑图。返回（新增页数，总页数）。 */
  galleryPageAdd: (
    id: number,
  ) => invoke<[number, number]>("gallery_page_add", { id }),

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

  /** 老板键第一步：把主窗藏起来。呼回键默认 Ctrl+Shift+H，可在设置里改。 */
  hideMainWindow: () => invoke<void>("hide_main_window"),

  /** 前端首帧就绪：亮主窗、关原生启动屏（splash.html）。 */
  splashFinish: () => invoke<void>("splash_finish"),

  /** 壁纸本地缓存文件名（null = 未缓存，前端退回原地址）。 */
  wallpaperCacheUrls: (walls: WallpaperSource[]) =>
    invoke<(string | null)[]>("wallpaper_cache_urls", { walls }),

  /** 老板键全局呼回键：读当前绑定（Rust 侧持久化 + 注册）。 */
  getBossRecall: () => invoke<string>("get_boss_recall"),

  /** 改老板键全局呼回键（Rust 注册成功才落盘）。 */
  setBossRecall: (binding: string) =>
    invoke<void>("set_boss_recall", { binding }),

  // ---------- 内置浏览器（工作台标签用；方案 docs/浏览器内置阅读器方案.md） ----------

  /** 挂载内置浏览器引擎（宿主 = main 窗），返回 label。 */
  d2EmbedOpen: (args: {
    url: string;
    adblock?: boolean;
    home?: string;
    picker?: boolean;
    /** 初始占位矩形（逻辑像素）：从第一帧就把工具栏摆到位，避免开新标签时顶栏错位。 */
    rect?: { x: number; y: number; w: number; h: number };
  }) => invoke<string>("d2_embed_open", args),

  /** 同步 React 占位矩形（逻辑像素）。 */
  d2EmbedSync: (args: { label: string; x: number; y: number; w: number; h: number }) =>
    invoke<void>("d2_embed_sync", args),

  /** 整体显隐（切标签 / 模态让位）。 */
  d2EmbedShow: (args: { label: string; visible: boolean }) =>
    invoke<void>("d2_embed_show", args),

  /** 拆除（只关 webview，不碰主窗）。 */
  d2EmbedClose: (label: string) => invoke<void>("d2_embed_close", { label }),

  /** 读浏览器设置（settings.json；主设置/⚙面板/D2 面板共用一份真源）。 */
  browserSettingsGet: () =>
    invoke<Record<string, unknown>>("browser_settings_get"),

  /** 补丁式写浏览器设置：只改传上来的字段，其余保留。 */
  browserSettingsSet: (patch: Record<string, unknown>) =>
    invoke<void>("browser_settings_set", { patch }),

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

  // ---------- 广告拦截（2026-11-26） ----------
  blockedList: () => invoke<BlockedEntry[]>("blocked_list"),
  blockedRemove: (host: string, selector: string) =>
    invoke<void>("blocked_remove", { host, selector }),
  blockedClear: () => invoke<void>("blocked_clear"),
  adblockReadCustom: () => invoke<string>("adblock_read_custom"),
  adblockWriteCustom: (content: string) =>
    invoke<void>("adblock_write_custom", { content }),

  // ---------- 浏览器数据（管理员 2026-11-25：历史 / 账号密码） ----------
  browserHistoryList: (search?: string, limit?: number) =>
    invoke<HistoryEntry[]>("browser_history_list", {
      search: search ?? null,
      limit: limit ?? null,
    }),
  browserHistoryDelete: (id: number) => invoke<void>("browser_history_delete", { id }),
  browserHistoryClear: () => invoke<void>("browser_history_clear"),
  browserCredentialList: () => invoke<CredentialEntry[]>("browser_credential_list"),
  /** id 传 null = 新增；password 传 null/空 = 不改密码。 */
  browserCredentialSave: (args: {
    id?: number | null;
    site: string;
    username: string;
    password?: string | null;
    note?: string | null;
  }) => invoke<number>("browser_credential_save", args),
  browserCredentialDelete: (id: number) => invoke<void>("browser_credential_delete", { id }),
  /** 按需解密取密码明文（DPAPI 绑当前 Windows 用户）。 */
  browserCredentialReveal: (id: number) =>
    invoke<string>("browser_credential_reveal", { id }),

  // ---------- 站点登录态（Cookie 快照，DPAPI 加密；下载时不依赖浏览器窗口） ----------
  /** 从当前浏览器抓某站点的 Cookie 并加密落盘，返回抓到的条数。 */
  siteLoginCapture: (host: string, note?: string) =>
    invoke<number>("site_login_capture", { host, note: note ?? "" }),
  siteLoginList: () => invoke<SiteLoginEntry[]>("site_login_list"),
  siteLoginDelete: (id: number) => invoke<void>("site_login_delete", { id }),
  /** 直接粘贴 Cookie 文本入加密库（不要求网页标签开着）。 */
  siteLoginImport: (host: string, cookies: string) =>
    invoke<number>("site_login_import", { host, cookies }),
  /** 看一条登录态快照里存了哪些 cookie 名（值不回显）。 */
  siteLoginDetail: (id: number) => invoke<string[]>("site_login_detail", { id }),
  /** 18comic 账号密码 API 登录（绕开网页 reCAPTCHA），成功返回写入的 cookie 数。 */
  jmApiLogin: (username: string, password: string) =>
    invoke<number>("jm_api_login", { username, password }),
  /** 用画匣专属配置的真 Edge（--app 窗）打开站点：登录一次永久有效。 */
  openInEdge: (url: string) => invoke<void>("open_in_edge", { url }),

  // ---------- 壁纸锁（管理员 2026-11-25；防君子不防小人） ----------
  /** 读壁纸锁配置（哈希/盐不下发，只给开关、壁纸列表、轮换间隔）。 */
  getWallpaperLock: () => invoke<WallpaperLockConfig>("get_wallpaper_lock"),
  saveWallpaperLock: (config: WallpaperLockConfig) =>
    invoke<WallpaperLockConfig>("save_wallpaper_lock", { config }),
  /** 设置/修改密码；空串 = 清除密码（默认无密码）。 */
  setWallpaperPassword: (password: string) =>
    invoke<WallpaperLockConfig>("set_wallpaper_password", { password }),
  verifyWallpaperPassword: (password: string) =>
    invoke<boolean>("verify_wallpaper_password", { password }),

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

/** 壁纸本地缓存 URL（文件名来自 wallpaperCacheUrls）。 */
export function wallpaperCacheFileUrl(name: string): string {
  return `${WCACHE_ORIGIN}/${name}`;
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

/** `wcache://`（壁纸本地缓存）在 Windows 上同样映射成 http 主机。 */
const WCACHE_ORIGIN =
  typeof navigator !== "undefined" && navigator.userAgent.includes("Windows")
    ? "http://wcache.localhost"
    : "wcache://localhost";

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
