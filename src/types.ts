export interface LibraryState {
  libraryPath: string | null;
  libraryName: string | null;
  archiveCount: number;
  missingCount: number;
  totalSize: number;
  lastLibraryPath: string | null;
  error: string | null;
}

export interface ArchiveCardData {
  id: number;
  title: string;
  fileName: string;
  ext: string;
  fileSize: number;
  rating: number;
  pageCount: number;
  addedAt: number;
  fileMtime: number;
  coverPath: string | null;
  filePath: string;
  note: string | null;
  missing: boolean;
  error: string | null;
  lastReadAt: number | null;
  lastPage: number;
  /** 打开过多少次；待看目录的加权挑选按它降权。 */
  readCount: number;
  groupBucket?: number;
  groupLabel?: string;
  groupCategory?: string;
}

export interface ArchiveGroupData {
  bucket: number;
  count: number;
  label?: string;
  category?: string;
}

export interface TrashedArchive {
  id: number;
  title: string;
  fileName: string;
  ext: string;
  fileSize: number;
  pageCount: number;
  lastReadAt: number | null;
  lastPage: number;
  missing: boolean;
  coverPath: string | null;
  deletedAt: number;
}

export interface ReaderInfo {
  pageCount: number;
  lastPage: number;
  bookmarks: ReaderBookmark[];
}

export interface ReaderBookmark {
  id: number;
  archiveId: number;
  page: number;
  createdAt: number;
}

/**
 * 单页元信息。图片本身由 `gallery://` 协议提供（见 api.readerPageUrl），
 * 不再随 IPC 传输 base64。
 */
export interface ReaderPage {
  index: number;
  name: string;
  /** 压缩包版本号（修改时间-大小），拼进取图 URL 做缓存键。 */
  version: string;
  /** 这一页的字节数，用来决定要不要预取。 */
  bytes: number;
}

/** 动图页（gif / 动画 webp）的帧元数据；静态页 animated=false。 */
export interface AnimMeta {
  animated: boolean;
  frames: number;
  /** 所有帧时长之和（毫秒），播放模式据此决定「播完翻页」的时刻。 */
  totalDurationMs: number;
  width: number;
  height: number;
}

/** 缩略图体积小，继续内联成 data URL，省一次协议往返。 */
export interface ReaderThumb {
  index: number;
  name: string;
  dataUrl: string;
}

export interface ArchivePage {
  items: ArchiveCardData[];
  total: number;
  page: number;
  pageSize: number;
  groups?: ArchiveGroupData[];
}

export interface Tag {
  id: number;
  number: number;
  name: string;
  originalName: string;
  translatedName: string | null;
  translationSource: string | null;
  translatedAt: number | null;
  category: string | null;
  mergedInto: number | null;
  deletedAt: number | null;
  count: number;
  /** 待看目录挑选权重。默认 100；0 表示不想看到带此标签的画集。 */
  weight: number;
}

export interface TagEdge {
  source: number;
  target: number;
  weight: number;
}

/** 批量导入时送往后端的一条标签。category 是已映射好的大类路径。 */
export interface TagDraft {
  name: string;
  category: string | null;
}

export interface BulkTagFailure {
  name: string;
  reason: string;
}

export interface BulkTagReport {
  added: number;
  linked: number;
  existing: number;
  categorized: number;
  failed: BulkTagFailure[];
}

export interface TagGraph {
  nodes: Tag[];
  edges: TagEdge[];
}

export interface Collection {
  id: number;
  name: string;
  isDefault: boolean;
  count: number;
  sortOrder: number;
  coverArchiveId: number | null;
  coverPath: string | null;
  deletedAt: number | null;
}

export type SortKey =
  | "added_at"
  | "file_mtime"
  | "rating"
  | "file_size"
  | "title"
  | "page_count"
  | "last_read_at";

/** 待看目录。以日期命名，最多同时存在 3 个。 */
export interface Queue {
  id: number;
  name: string;
  createdAt: number;
  /** 目录里的画集总数。 */
  count: number;
  /** 其中已读完的本数。 */
  finished: number;
}

/** 生成待看目录的选项。 */
export interface QueueFillOptions {
  /** 每个目录放几本，默认 10。 */
  size?: number;
  /** 只在这批标签里挑（并集）；为空表示全库。 */
  tagIds?: number[];
  /** 是否允许挑已经看过的本，默认 false。 */
  includeRead?: boolean;
}

export interface QueueFillReport {
  queue: Queue;
  added: number;
  /** 候选池里被 0 权重标签排除掉的本数。 */
  excludedByWeight: number;
  candidates: number;
  titles: string[];
}

/** 「退出后自动发布」的状态。 */
export interface ReleaseStatus {
  /** 已经排好一次发布，等应用退出。 */
  pending: boolean;
  /** success / failed / running / none */
  lastResult: string;
  lastAt: number;
  lastMessage: string;
  scriptPath: string;
  logPath: string;
}

export interface ReleaseState {
  /** 画匣源码目录（含 scripts/release.mjs）。 */
  sourceDir: string | null;
  autoOnExit: boolean;
  relaunchAfter: boolean;
  status: ReleaseStatus;
  /** 下次关窗是否会真的发布（有实质更新才发） */
  readiness?: {
    hasBaseline: boolean;
    lastVersion: string;
    lastHead: string;
    currentHead: string;
    dirty: boolean;
    hasChanges: boolean;
    reason: string;
  };
}

export type SortDirection = "asc" | "desc";

export type ViewMode = "scroll" | "pages";

export type TagFilterMode = "and" | "or" | "exclude";

export type ReadStatus = "unread" | "reading" | "finished";

export type GroupMode = "none" | "rating" | "tag" | "collection";

export interface TagCategory {
  id: number;
  name: string;
  parentId: number | null;
  path: string;
  depth: number;
}

export interface ArchiveQuery {
  page?: number;
  pageSize?: number;
  sort?: SortKey;
  direction?: SortDirection;
  collectionId?: number;
  /** 只看某个待看目录里的画集。 */
  queueId?: number;
  tagIds?: number[];
  tagMode?: TagFilterMode;
  ratingBuckets?: number[];
  readStatuses?: ReadStatus[];
  missingOnly?: boolean;
  search?: string;
  group?: GroupMode;
}

export interface TagTranslationInput {
  id: number;
  translatedName: string | null;
  source?: string;
}

export interface ImportOptions {
  addToCollection?: number;
  preserveStructure?: boolean;
  /** 导入成功后挂到每一本新画集上的标签。 */
  tags?: TagDraft[];
}

/** 导入面板里的一行：拖进来之后、点确认之前的状态。 */
export interface StageItem {
  /** 源文件绝对路径，导入时原样回传。 */
  path: string;
  fileName: string;
  /** 去掉扩展名，作为标题初值展示。 */
  stem: string;
  extension: string;
  sizeBytes: number;
  /** 压缩包里的图片张数；批量较大时后端不数，为 null。 */
  pageCount: number | null;
  /** 库里已有同名同大小的记录，导入时会被跳过。 */
  duplicate: boolean;
  /** 文件本身就在库目录内，导入只建索引、不复制。 */
  inLibrary: boolean;
  /** 无法导入的原因。 */
  issue: string | null;
}

export interface StageReport {
  items: StageItem[];
  errors: ImportErrorItem[];
  totalBytes: number;
}

export interface ImportProgress {
  current: number;
  total: number;
  fileName: string;
  imported: number;
  skipped: number;
  failed: number;
  phase: string;
}

export interface ImportErrorItem {
  file: string;
  message: string;
}

export interface ImportReport {
  imported: number;
  skipped: number;
  failed: number;
  errors: ImportErrorItem[];
  message: string;
}

/** D2 M1：一次受控 WebView 抓取并入库的结果。 */
export interface D2FetchReport {
  title: string;
  sourceUrl: string;
  pageUrl: string;
  imageUrl: string;
  cbzPath: string;
  importReport: ImportReport;
}

/** D2：`d2://done` 事件负载。 */
export interface D2Done {
  ok: boolean;
  stage: string;
  report: D2FetchReport | null;
  error: string | null;
}

export interface ScanReport {
  found: number;
  added: number;
  updated: number;
  missing: number;
  errors: string[];
  elapsedMs: number;
  message: string;
}
