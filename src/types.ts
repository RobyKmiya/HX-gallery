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
