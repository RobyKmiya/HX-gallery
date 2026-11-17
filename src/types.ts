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

export interface ScanReport {
  found: number;
  added: number;
  updated: number;
  missing: number;
  errors: string[];
  elapsedMs: number;
  message: string;
}
