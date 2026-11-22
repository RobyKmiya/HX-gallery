import {
  Fragment,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useReducer,
  useRef,
  useState,
} from "react";
import { getVersion } from "@tauri-apps/api/app";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import { api, assetUrl } from "../api";
import { usePersistentFlag, usePersistentSize } from "../layout";
import { formatBytes } from "../format";
import type {
  ArchiveCardData,
  ArchivePage,
  Collection,
  GroupMode,
  ImportProgress,
  ImportReport,
  LibraryState,
  Queue,
  ReadStatus,
  SeriesSummary,
  SortDirection,
  SortKey,
  Tag,
  TagFilterMode,
  ViewMode,
} from "../types";
import ArchiveCard from "./ArchiveCard";
import CollectionEditor from "./CollectionEditor";
import CollectionNameDialog from "./CollectionNameDialog";
import DetailDrawer from "./DetailDrawer";
import ImportDialog, {
  type ImportDialogHandle,
  type ImportSubmit,
} from "./ImportDialog";
import ImportResultDialog from "./ImportResultDialog";
import LoosePanel from "./LoosePanel";
import QueuePanel from "./QueuePanel";
import SeriesCard from "./SeriesCard";
import SeriesDrawer from "./SeriesDrawer";
import SettingsDialog from "./SettingsDialog";
import CollectionGlyph, {
  COLLECTION_COLORS,
  COLLECTION_ICONS,
} from "./CollectionGlyph";
import ReaderMenu from "./ReaderMenu";
import type { CollectionIcon } from "../types";
import Reader from "./Reader";
import Splitter from "./Splitter";
import ReleasePanel from "./ReleasePanel";
import D2Panel from "./D2Panel";
import TagCenter from "./TagCenter";
import Toast, { type ToastData, type ToastKind } from "./Toast";
import TrashCenter from "./TrashCenter";
import {
  AlertIcon,
  BookIcon,
  CheckIcon,
  ChevronLeftIcon,
  ChevronRightIcon,
  CloseIcon,
  FolderIcon,
  FilterIcon,
  GridIcon,
  HeartIcon,
  ImportIcon,
  LogoIcon,
  PauseIcon,
  PlayIcon,
  PlusIcon,
  RefreshIcon,
  SearchIcon,
  StarIcon,
  SwapIcon,
  TagIcon,
  TrashIcon,
} from "./Icons";

interface NavSnapshot {
  collectionId: number | null;
  tagCenter: boolean;
  trash: boolean;
}

interface LibraryViewProps {
  library: LibraryState;
  onLibraryChanged: () => Promise<LibraryState>;
  onSwitchLibrary: () => void;
}

const SORT_OPTIONS: { value: SortKey; label: string }[] = [
  { value: "added_at", label: "最近添加" },
  { value: "last_read_at", label: "最近阅读" },
  { value: "file_mtime", label: "文件修改时间" },
  { value: "rating", label: "评分" },
  { value: "file_size", label: "文件大小" },
  { value: "title", label: "标题" },
  { value: "page_count", label: "页数" },
];

const PAGE_SIZES = [24, 48, 96];
const AUTO_PAGE_SECONDS = [5, 8, 12];
const COVER_PRELOAD_TIMEOUT_MS = 900;
const PAGE_CACHE_LIMIT = 6;
const WARM_COVER_LIMIT = 128;

const UI_PREFS_PREFIX = "archive-gallery:ui-prefs:v1:";

interface UiPrefs {
  sort?: SortKey;
  direction?: SortDirection;
  viewMode?: ViewMode;
  pageSize?: number;
  tagMode?: TagFilterMode;
  tagIds?: unknown;
  ratingBuckets?: unknown;
  readStatuses?: unknown;
  missingOnly?: unknown;
  search?: unknown;
  group?: GroupMode;
  autoSeconds?: number;
  detailDock?: "dock" | "float";
}

const RATING_GROUP_LABELS: Record<number, string> = {
  5: "81–100 分",
  4: "61–80 分",
  3: "41–60 分",
  2: "21–40 分",
  1: "1–20 分",
  0: "未评分",
};

const RATING_FILTER_OPTIONS: { bucket: number; label: string }[] = [
  { bucket: 0, label: "未评分" },
  { bucket: 1, label: "1–20" },
  { bucket: 2, label: "21–40" },
  { bucket: 3, label: "41–60" },
  { bucket: 4, label: "61–80" },
  { bucket: 5, label: "81–100" },
];

const READ_STATUS_OPTIONS: { value: ReadStatus; label: string }[] = [
  { value: "unread", label: "未读" },
  { value: "reading", label: "阅读中" },
  { value: "finished", label: "已读完" },
];

function ratingBucket(rating: number): number {
  return rating <= 0 ? 0 : Math.floor((rating + 19) / 20);
}

function uiPrefsKey(libraryPath: string | null): string {
  return `${UI_PREFS_PREFIX}${libraryPath ?? "default"}`;
}

function readUiPrefs(key: string): UiPrefs {
  try {
    const raw = window.localStorage.getItem(key);
    if (!raw) return {};
    const value: unknown = JSON.parse(raw);
    return value && typeof value === "object" ? (value as UiPrefs) : {};
  } catch {
    return {};
  }
}

function isSupportedDropPath(path: string): boolean {
  if (/\.(zip|cbz|7z|cb7|rar|cbr)$/i.test(path)) return true;
  const name = path.split(/[\\/]/).pop() ?? "";
  return !name.includes(".");
}

async function preloadCoverImages(cards: ArchiveCardData[]): Promise<void> {
  const urls = Array.from(
    new Set(
      cards
        .map((card) => (card.coverPath ? assetUrl(card.coverPath) : null))
        .filter((url): url is string => url !== null),
    ),
  );
  if (urls.length === 0) return;

  let timer = 0;
  const timeout = new Promise<void>((resolve) => {
    timer = window.setTimeout(resolve, COVER_PRELOAD_TIMEOUT_MS);
  });
  const loads = urls.map(
    (url) =>
      new Promise<void>((resolve) => {
        const image = new Image();
        image.onload = () => {
          if (typeof image.decode === "function") {
            image.decode().then(
              () => resolve(),
              () => resolve(),
            );
          } else {
            resolve();
          }
        };
        image.onerror = () => resolve();
        image.src = url;
      }),
  );

  await Promise.race([Promise.all(loads).then(() => undefined), timeout]);
  window.clearTimeout(timer);
}

function trimPageCache(cache: Map<number, ArchivePage>, current: number) {
  if (cache.size <= PAGE_CACHE_LIMIT) return;
  for (const key of [...cache.keys()]) {
    if (cache.size <= PAGE_CACHE_LIMIT) return;
    if (Math.abs(key - current) > 1) cache.delete(key);
  }
}

export default function LibraryView({
  library,
  onLibraryChanged,
  onSwitchLibrary,
}: LibraryViewProps) {
  const [items, setItems] = useState<ArchiveCardData[]>([]);
  /** 合集卡：只在纯浏览第 1 页有值，其余情况为空数组。 */
  const [seriesList, setSeriesList] = useState<SeriesSummary[]>([]);
  const [openSeries, setOpenSeries] = useState<SeriesSummary | null>(null);
  const [seriesCreateOpen, setSeriesCreateOpen] = useState(false);
  const [seriesBusy, setSeriesBusy] = useState(false);
  /** 统一设置对话框。 */
  const [settingsOpen, setSettingsOpen] = useState(false);
  /** 主目录空白处右键菜单。 */
  const [bgMenu, setBgMenu] = useState<{ x: number; y: number } | null>(null);
  /** 收藏夹样式编辑弹窗（图标形状 + 填充色）。 */
  const [styleEditing, setStyleEditing] = useState<{ id: number; x: number; y: number } | null>(
    null,
  );
  const [warmCovers, setWarmCovers] = useState<string[]>([]);
  const [total, setTotal] = useState(0);
  const [page, setPage] = useState(1);
  const prefsKey = uiPrefsKey(library.libraryPath);
  const [savedPrefs] = useState(() => readUiPrefs(prefsKey));
  const [pageSize, setPageSize] = useState(
    PAGE_SIZES.includes(savedPrefs.pageSize ?? -1)
      ? (savedPrefs.pageSize as number)
      : 48,
  );
  const [sort, setSort] = useState<SortKey>(
    SORT_OPTIONS.some((option) => option.value === savedPrefs.sort)
      ? (savedPrefs.sort as SortKey)
      : "added_at",
  );
  const [direction, setDirection] = useState<SortDirection>(
    savedPrefs.direction === "asc" ? "asc" : "desc",
  );
  const [viewMode, setViewMode] = useState<ViewMode>(
    savedPrefs.viewMode === "pages" ? "pages" : "scroll",
  );
  const [autoPaging, setAutoPaging] = useState(false);
  const [autoPageSeconds, setAutoPageSeconds] = useState(
    AUTO_PAGE_SECONDS.includes(savedPrefs.autoSeconds ?? -1)
      ? (savedPrefs.autoSeconds as number)
      : 8,
  );
  const [loading, setLoading] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [listError, setListError] = useState<string | null>(null);
  const [scanning, setScanning] = useState(false);
  const [importing, setImporting] = useState(false);
  const [importProgress, setImportProgress] = useState<ImportProgress | null>(
    null,
  );
  const [importReport, setImportReport] = useState<ImportReport | null>(null);
  /** 导入面板：先暂存、再确认，避免拖错文件就直接写进库里。 */
  const [importOpen, setImportOpen] = useState(false);
  const [importSeedPaths, setImportSeedPaths] = useState<string[]>([]);
  const importDialogRef = useRef<ImportDialogHandle>(null);
  const [selected, setSelected] = useState<ArchiveCardData | null>(null);
  const [readerCard, setReaderCard] = useState<ArchiveCardData | null>(null);
  /** 合集连读：正在读的合集成员顺序（archiveId 列表）。从合集面板打开时设置。 */
  const [readerSeriesIds, setReaderSeriesIds] = useState<number[] | null>(
    null,
  );
  /** 详情面板形态：dock = 停靠在画廊旁（外拓），float = 悬浮遮罩。 */
  const [detailDock, setDetailDock] = useState<"dock" | "float">(
    savedPrefs.detailDock === "float" ? "float" : "dock",
  );
  /** 在阅读器里打开的详情浮层。 */
  const [readerDetailOpen, setReaderDetailOpen] = useState(false);
  const [toast, setToast] = useState<ToastData | null>(null);
  /** 应用版本号（来自安装包），显示在侧栏底部，一眼确认有没有更新。 */
  const [appVersion, setAppVersion] = useState("");
  const [collections, setCollections] = useState<Collection[]>([]);
  const [activeCollectionId, setActiveCollectionId] = useState<number | null>(
    null,
  );
  const [collectionCreateIntent, setCollectionCreateIntent] = useState<
    null | "browse" | "batch"
  >(null);
  const [savingCollection, setSavingCollection] = useState(false);
  const [tags, setTags] = useState<Tag[]>([]);
  const [activeTagIds, setActiveTagIds] = useState<number[]>(
    Array.isArray(savedPrefs.tagIds)
      ? savedPrefs.tagIds.filter(
          (id): id is number => Number.isInteger(id) && (id as number) > 0,
        )
      : [],
  );
  const [tagMode, setTagMode] = useState<TagFilterMode>(
    savedPrefs.tagMode === "or" || savedPrefs.tagMode === "exclude"
      ? savedPrefs.tagMode
      : "and",
  );
  const [ratingBuckets, setRatingBuckets] = useState<number[]>(
    Array.isArray(savedPrefs.ratingBuckets)
      ? Array.from(
          new Set(
            savedPrefs.ratingBuckets.filter(
              (bucket): bucket is number =>
                Number.isInteger(bucket) &&
                (bucket as number) >= 0 &&
                (bucket as number) <= 5,
            ),
          ),
        )
      : [],
  );
  const [readStatuses, setReadStatuses] = useState<ReadStatus[]>(
    Array.isArray(savedPrefs.readStatuses)
      ? Array.from(
          new Set(
            savedPrefs.readStatuses.filter(
              (status): status is ReadStatus =>
                status === "unread" ||
                status === "reading" ||
                status === "finished",
            ),
          ),
        )
      : [],
  );
  const [missingOnly, setMissingOnly] = useState(
    savedPrefs.missingOnly === true,
  );
  const [tagFilterOpen, setTagFilterOpen] = useState(false);
  const [filterOpen, setFilterOpen] = useState(false);
  const [tagQuery, setTagQuery] = useState("");
  const [tagCenterOpen, setTagCenterOpen] = useState(false);
  const [trashOpen, setTrashOpen] = useState(false);
  const [queuePanelOpen, setQueuePanelOpen] = useState(false);
  const [looseView, setLooseView] = useState(false);
  /** 布局尺寸：侧栏宽度与停靠详情宽度，拖动分界线改、存在本机。 */
  // min 必须跟 Splitter 的 min 一致（62 = 纯图标条宽度）。之前的 190 会把
  // 拖动结果夹回去，导致 is-tight/is-compact 永远不触发（22:11 反馈的真凶）。
  const [sidebarWidth, setSidebarWidth, resetSidebarWidth] = usePersistentSize(
    "huaxia.layout.sidebar",
    262,
    62,
    420,
  );
  const [drawerWidth, setDrawerWidth, resetDrawerWidth] = usePersistentSize(
    "huaxia.layout.drawer",
    430,
    320,
    760,
  );
  /** 侧栏折叠成图标条（双击分界线或点侧栏底部按钮切换）。 */
  const [sidebarCollapsed, setSidebarCollapsed] = usePersistentFlag(
    "huaxia.layout.sidebarCollapsed",
    false,
  );
  const toggleSidebarCollapsed = useCallback(
    () => setSidebarCollapsed(!sidebarCollapsed),
    [setSidebarCollapsed, sidebarCollapsed],
  );
  const SIDEBAR_RAIL_WIDTH = 62;

  /** 侧栏「收藏夹」大类是否折叠（记在本机，重开也保持）。 */
  const [collectionsCollapsed, setCollectionsCollapsed] = useState(() => {
    try {
      return localStorage.getItem("huaxia.collectionsCollapsed") === "1";
    } catch {
      return false;
    }
  });
  const toggleCollectionsCollapsed = useCallback(() => {
    setCollectionsCollapsed((previous) => {
      const next = !previous;
      try {
        localStorage.setItem("huaxia.collectionsCollapsed", next ? "1" : "0");
      } catch {
        // 存不了就只在本次生效
      }
      return next;
    });
  }, []);
  const [releasePanelOpen, setReleasePanelOpen] = useState(false);
  const [d2PanelOpen, setD2PanelOpen] = useState(false);
  /**
   * 正在浏览的待看目录。非空时列表只显示这个目录的成员；
   * 它和收藏夹是两套筛选，进待看目录时把收藏夹选择清掉。
   */
  const [activeQueue, setActiveQueue] = useState<Queue | null>(null);

  // 换了收藏夹、切了视图、或打开了别的面板：合集详情自动收起（管理员 11-22：
  // 「点了合集再点画廊不会关」）。
  useEffect(() => {
    setOpenSeries(null);
  }, [
    activeCollectionId,
    activeQueue?.id,
    looseView,
    tagCenterOpen,
    queuePanelOpen,
    trashOpen,
    settingsOpen,
    viewMode,
  ]);

  /** 打开待看目录：切到它的成员列表，并退出收藏夹视图。 */
  const openQueue = useCallback((queue: Queue) => {
    setActiveCollectionId(null);
    setActiveQueue(queue);
    setQueuePanelOpen(false);
  }, []);

  /** 回到全部画集。 */
  const clearActiveQueue = useCallback(() => {
    setActiveQueue(null);
  }, []);
  const [search, setSearch] = useState(
    typeof savedPrefs.search === "string" ? savedPrefs.search : "",
  );
  const [searchTerm, setSearchTerm] = useState(
    typeof savedPrefs.search === "string" ? savedPrefs.search : "",
  );
  const [groupBy, setGroupBy] = useState<GroupMode>(
    savedPrefs.group === "rating" ||
      savedPrefs.group === "tag" ||
      savedPrefs.group === "collection"
      ? savedPrefs.group
      : "none",
  );
  const [groupCounts, setGroupCounts] = useState<Record<
    number,
    number
  > | null>(null);
  const [dragActive, setDragActive] = useState(false);
  const [quickEditCard, setQuickEditCard] = useState<ArchiveCardData | null>(
    null,
  );
  const [selectionMode, setSelectionMode] = useState(false);
  const [selectedIds, setSelectedIds] = useState<number[]>([]);
  const [batchCollectionId, setBatchCollectionId] = useState<number | "">("");
  const [batchQueueId, setBatchQueueId] = useState<number | "">("");
  const [queues, setQueues] = useState<Queue[]>([]);
  const [batchBusy, setBatchBusy] = useState(false);
  const [batchDeleteConfirm, setBatchDeleteConfirm] = useState(false);
  /** 卡片右键菜单：定位 + 目标卡（null = 关闭）。 */
  const [cardMenu, setCardMenu] = useState<{
    card: ArchiveCardData;
    x: number;
    y: number;
  } | null>(null);

  const navHistoryRef = useRef<NavSnapshot[]>([]);
  const navIndexRef = useRef(0);
  const lastSideNavAtRef = useRef(0);
  const [, refreshNav] = useReducer((value: number) => value + 1, 0);

  const navSnapshot = useMemo<NavSnapshot>(
    () => ({
      collectionId: activeCollectionId,
      tagCenter: tagCenterOpen,
      trash: trashOpen,
    }),
    [activeCollectionId, tagCenterOpen, trashOpen],
  );

  useEffect(() => {
    const history = navHistoryRef.current;
    const current = history[navIndexRef.current];
    if (
      current &&
      current.collectionId === navSnapshot.collectionId &&
      current.tagCenter === navSnapshot.tagCenter &&
      current.trash === navSnapshot.trash
    ) {
      return;
    }

    history.splice(navIndexRef.current + 1);
    history.push(navSnapshot);
    if (history.length > 60) history.shift();
    navIndexRef.current = history.length - 1;
    refreshNav();
  }, [navSnapshot]);

  const applyNavSnapshot = useCallback((snapshot: NavSnapshot) => {
    setActiveCollectionId(snapshot.collectionId);
    setTagCenterOpen(snapshot.tagCenter);
    setTrashOpen(snapshot.trash);
  }, []);

  /**
   * 后退是「分层退出」：先关最上层的浮层（阅读器 → 详情 → 快速编辑），
   * 全关了才走集合级历史。快照里只存集合级状态，所以打开/关闭阅读器、
   * 阅读器里换书都不会压历史——后退永远是「回上一级」，不会回放上一本。
   */
  const goBack = useCallback(() => {
    if (readerCard) {
      setReaderCard(null);
      return;
    }
    if (selected) {
      setSelected(null);
      return;
    }
    if (quickEditCard) {
      setQuickEditCard(null);
      return;
    }
    if (navIndexRef.current <= 0) return;
    navIndexRef.current -= 1;
    applyNavSnapshot(navHistoryRef.current[navIndexRef.current]);
    refreshNav();
  }, [readerCard, selected, quickEditCard, applyNavSnapshot]);

  const goForward = useCallback(() => {
    if (navIndexRef.current >= navHistoryRef.current.length - 1) return;
    navIndexRef.current += 1;
    applyNavSnapshot(navHistoryRef.current[navIndexRef.current]);
    refreshNav();
  }, [applyNavSnapshot]);

  useEffect(() => {
    const handleSideButton = (event: MouseEvent) => {
      if (event.button !== 3 && event.button !== 4) return;
      event.preventDefault();
      event.stopPropagation();

      const now = Date.now();
      if (now - lastSideNavAtRef.current < 250) return;
      lastSideNavAtRef.current = now;

      // 阅读器开着时侧键归阅读器：沉浸态先退出沉浸，非沉浸才退出阅读。
      // 用自定义事件派发，避免 LibraryView 与 Reader 争抢同一个 mouseup。
      const readerEl = document.querySelector(".reader");
      if (readerEl) {
        window.dispatchEvent(
          new CustomEvent(
            readerEl.classList.contains("is-immersive")
              ? "huaxia:exit-immersive"
              : "huaxia:close-reader",
          ),
        );
        return;
      }

      if (event.button === 3) goBack();
      else goForward();
    };

    const blockSideButton = (event: MouseEvent) => {
      if (event.button === 3 || event.button === 4) {
        event.preventDefault();
        event.stopPropagation();
      }
    };

    window.addEventListener("mousedown", blockSideButton, true);
    window.addEventListener("mouseup", handleSideButton, true);
    window.addEventListener("auxclick", handleSideButton, true);
    return () => {
      window.removeEventListener("mousedown", blockSideButton, true);
      window.removeEventListener("mouseup", handleSideButton, true);
      window.removeEventListener("auxclick", handleSideButton, true);
    };
  }, [goBack, goForward]);

  const canGoBack = navIndexRef.current > 0;
  const canGoForward =
    navIndexRef.current < navHistoryRef.current.length - 1;
  const [renamingCollectionId, setRenamingCollectionId] = useState<
    number | null
  >(null);
  const [renameDraft, setRenameDraft] = useState("");
  const [renameBusy, setRenameBusy] = useState(false);
  const [pendingDeleteCollectionId, setPendingDeleteCollectionId] = useState<
    number | null
  >(null);

  const requestIdRef = useRef(0);
  const queryTokenRef = useRef(0);
  const prefetchGenRef = useRef(0);
  const pageCacheRef = useRef<Map<number, ArchivePage>>(new Map());
  const decodedPagesRef = useRef<Set<number>>(new Set());
  const prefetchPagesRef = useRef<Set<number>>(new Set());
  const tagFilterRef = useRef<HTMLDivElement | null>(null);
  const filterRef = useRef<HTMLDivElement | null>(null);
  const contentRef = useRef<HTMLDivElement | null>(null);
  const pageRef = useRef(1);
  const totalPagesRef = useRef(1);
  const viewModeRef = useRef<ViewMode>(viewMode);
  const itemsRef = useRef<ArchiveCardData[]>([]);
  const loadingRef = useRef(false);
  const loadingMoreRef = useRef(false);
  const sentinelRef = useRef<HTMLDivElement | null>(null);
  const initialScanStartedRef = useRef(false);

  const notify = useCallback((message: string, kind: ToastKind = "info") => {
    setToast({ id: Date.now(), message, kind });
  }, []);

  // 开场把「上次更新到底成没成」说清楚，别让人自己去猜。
  // 同一次失败只提示一次（按 lastAt 去重），否则每次开都弹会很烦。
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const state = await api.getReleaseState();
        if (cancelled) return;
        const status = state.status;
        if (status.lastResult !== "failed" && status.lastResult !== "running") return;
        const shownAt = (() => {
          try {
            return localStorage.getItem("huaxia.release.lastFailureShown");
          } catch {
            return null;
          }
        })();
        if (shownAt === String(status.lastAt)) return;
        try {
          localStorage.setItem("huaxia.release.lastFailureShown", String(status.lastAt));
        } catch {
          // 存不了就每次都说
        }
        notify(
          status.lastResult === "running"
            ? "上次更新被中断了，当前还是已安装的版本（可在「更新画匣」里重试）"
            : `上次更新没成功：${status.lastMessage || "构建或安装出错"}（当前仍是已安装的版本）`,
          "error",
        );
      } catch {
        // 读不到更新状态就安静
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [notify]);

  const patchCachedCards = useCallback(
    (patch: (card: ArchiveCardData) => ArchiveCardData) => {
      for (const result of pageCacheRef.current.values()) {
        result.items = result.items.map(patch);
      }
    },
    [],
  );

  const loadCollections = useCallback(async () => {
    try {
      setCollections(await api.listCollections());
    } catch (error) {
      notify(`收藏夹加载失败：${String(error)}`, "error");
    }
  }, [notify]);

  const loadTags = useCallback(async () => {
    try {
      const next = await api.listTags();
      setTags(next);
      setActiveTagIds((previous) => {
        const kept = previous.filter((id) =>
          next.some((tag) => tag.id === id),
        );
        return kept.length === previous.length ? previous : kept;
      });
    } catch (error) {
      notify(`标签加载失败：${String(error)}`, "error");
    }
  }, [notify]);

  useEffect(() => {
    void loadCollections();
  }, [loadCollections]);

  useEffect(() => {
    void loadTags();
  }, [loadTags]);

  useEffect(() => {
    const timer = window.setTimeout(() => setSearchTerm(search.trim()), 320);
    return () => window.clearTimeout(timer);
  }, [search]);

  useEffect(() => {
    itemsRef.current = items;
  }, [items]);

  useEffect(() => {
    pageRef.current = page;
  }, [page]);

  useEffect(() => {
    viewModeRef.current = viewMode;
  }, [viewMode]);

  useEffect(() => {
    totalPagesRef.current = Math.max(1, Math.ceil(total / pageSize));
  }, [pageSize, total]);

  useEffect(() => {
    if (!tagFilterOpen) return;
    const handlePointerDown = (event: MouseEvent) => {
      if (!tagFilterRef.current?.contains(event.target as Node)) {
        setTagFilterOpen(false);
      }
    };
    document.addEventListener("mousedown", handlePointerDown);
    return () => document.removeEventListener("mousedown", handlePointerDown);
  }, [tagFilterOpen]);

  useEffect(() => {
    if (!filterOpen) return;
    const handlePointerDown = (event: MouseEvent) => {
      if (!filterRef.current?.contains(event.target as Node)) {
        setFilterOpen(false);
      }
    };
    document.addEventListener("mousedown", handlePointerDown);
    return () => document.removeEventListener("mousedown", handlePointerDown);
  }, [filterOpen]);

  useEffect(() => {
    try {
      window.localStorage.setItem(
        prefsKey,
        JSON.stringify({
          sort,
          direction,
          viewMode,
          pageSize,
          tagMode,
          tagIds: activeTagIds,
          ratingBuckets,
          readStatuses,
          missingOnly,
          search,
          group: groupBy,
          autoSeconds: autoPageSeconds,
          detailDock,
        }),
      );
    } catch {
      // 本地存储不可用时只是不持久化，不影响当前会话
    }
  }, [
    prefsKey,
    sort,
    direction,
    viewMode,
    pageSize,
    tagMode,
    activeTagIds,
    ratingBuckets,
    readStatuses,
    missingOnly,
    search,
    groupBy,
    autoPageSeconds,
    detailDock,
  ]);

  // 关掉阅读器时把「阅读器里的详情」一并收起。
  useEffect(() => {
    if (!readerCard) setReaderDetailOpen(false);
  }, [readerCard]);

  // 版本号取自安装包（tauri.conf.json），只取一次；失败就静默，不影响别的功能。
  useEffect(() => {
    let cancelled = false;
    getVersion()
      .then((version) => {
        if (!cancelled && version) setAppVersion(version);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, []);

  const updateRating = useCallback(
    async (id: number, rating: number) => {
      await api.setRating(id, rating);
      const patch = (card: ArchiveCardData) =>
        card.id === id ? { ...card, rating } : card;
      patchCachedCards(patch);
      setItems((previous) => previous.map(patch));
      setSelected((current) =>
        current && current.id === id ? { ...current, rating } : current,
      );
    },
    [patchCachedCards],
  );

  const queryPage = useCallback(
    (targetPage: number) =>
      api.queryArchives({
        page: targetPage,
        pageSize,
        sort,
        direction,
        collectionId: activeCollectionId ?? undefined,
        queueId: activeQueue?.id ?? undefined,
        tagIds: activeTagIds.length > 0 ? activeTagIds : undefined,
        tagMode: activeTagIds.length > 0 ? tagMode : undefined,
        ratingBuckets: ratingBuckets.length > 0 ? ratingBuckets : undefined,
        readStatuses: readStatuses.length > 0 ? readStatuses : undefined,
        missingOnly: missingOnly || undefined,
        search: searchTerm || undefined,
        group: groupBy === "none" ? undefined : groupBy,
      }),
    [
      activeCollectionId,
      activeQueue,
      activeTagIds,
      direction,
      groupBy,
      missingOnly,
      pageSize,
      ratingBuckets,
      readStatuses,
      searchTerm,
      sort,
      tagMode,
    ],
  );

  const refreshWarmCovers = useCallback((center: number) => {
    const urls: string[] = [];
    const seen = new Set<string>();
    for (const target of [center - 1, center + 1]) {
      const cached = pageCacheRef.current.get(target);
      if (!cached) continue;
      for (const card of cached.items) {
        if (urls.length >= WARM_COVER_LIMIT) break;
        if (!card.coverPath) continue;
        const url = assetUrl(card.coverPath);
        if (seen.has(url)) continue;
        seen.add(url);
        urls.push(url);
      }
    }
    setWarmCovers((previous) =>
      previous.length === urls.length &&
      previous.every((url, index) => url === urls[index])
        ? previous
        : urls,
    );
  }, []);

  const commitPage = useCallback(
    (result: ArchivePage, targetPage: number) => {
      setGroupCounts(
        result.groups
          ? Object.fromEntries(
              result.groups.map((group) => [group.bucket, group.count]),
            )
          : null,
      );
      setItems(result.items);
      setSeriesList(result.series ?? []);
      setTotal(result.total);
      setPage(targetPage);
      refreshWarmCovers(targetPage);
    },
    [refreshWarmCovers],
  );

  const prefetchPage = useCallback(
    async (targetPage: number) => {
      const cached = pageCacheRef.current.get(targetPage);
      if (cached) {
        if (!decodedPagesRef.current.has(targetPage)) {
          decodedPagesRef.current.add(targetPage);
          void preloadCoverImages(cached.items);
        }
        return;
      }
      if (prefetchPagesRef.current.has(targetPage)) return;
      const token = queryTokenRef.current;
      const gen = prefetchGenRef.current;
      prefetchPagesRef.current.add(targetPage);
      try {
        await new Promise((resolve) => window.setTimeout(resolve, 80));
        if (pageCacheRef.current.has(targetPage)) return;
        if (token !== queryTokenRef.current) return;
        if (gen !== prefetchGenRef.current) return;
        const result = await queryPage(targetPage);
        if (gen !== prefetchGenRef.current) return;
        await preloadCoverImages(result.items);
        if (token !== queryTokenRef.current) return;
        if (gen !== prefetchGenRef.current) return;
        decodedPagesRef.current.add(targetPage);
        pageCacheRef.current.set(targetPage, result);
        trimPageCache(pageCacheRef.current, pageRef.current);
        if (Math.abs(targetPage - pageRef.current) === 1) {
          refreshWarmCovers(pageRef.current);
        }
      } catch {
        // Neighbour prefetch is best-effort; the visible page is unaffected.
      } finally {
        prefetchPagesRef.current.delete(targetPage);
      }
    },
    [queryPage, refreshWarmCovers],
  );

  const prefetchAround = useCallback(
    (center: number, totalItems: number) => {
      if (viewModeRef.current !== "pages") return;
      const maxPage = Math.max(1, Math.ceil(totalItems / pageSize));
      for (const target of [center + 1, center + 2, center - 1]) {
        if (target >= 1 && target <= maxPage) void prefetchPage(target);
      }
    },
    [pageSize, prefetchPage],
  );

  const loadPage = useCallback(
    async (targetPage: number, replace: boolean, fresh = false) => {
      const requestId = ++requestIdRef.current;
      const token = queryTokenRef.current;
      if (replace) prefetchGenRef.current += 1;
      if (fresh) {
        pageCacheRef.current.delete(targetPage);
        decodedPagesRef.current.delete(targetPage);
      }
      setListError(null);

      if (replace) {
        const cached = fresh
          ? undefined
          : pageCacheRef.current.get(targetPage);
        if (cached) {
          loadingRef.current = false;
          loadingMoreRef.current = false;
          setLoading(false);
          setLoadingMore(false);
          if (!decodedPagesRef.current.has(targetPage)) {
            decodedPagesRef.current.add(targetPage);
            void preloadCoverImages(cached.items);
          }
          commitPage(cached, targetPage);
          prefetchAround(targetPage, cached.total);
          return;
        }
        loadingRef.current = true;
        setLoading(true);
      } else {
        loadingMoreRef.current = true;
        setLoadingMore(true);
      }

      try {
        const result = await queryPage(targetPage);
        if (requestId !== requestIdRef.current) return;
        if (token !== queryTokenRef.current) return;

        if (replace) {
          pageCacheRef.current.set(targetPage, result);
          trimPageCache(pageCacheRef.current, targetPage);
          /*
           * 先换页、再预热封面。
           *
           * 原来是「等这一页所有封面 load + decode 完（最多 900ms）才提交」，
           * 于是按方向键 / 滚轮翻页时，界面会先卡住一段时间再整页刷新。
           * 卡片自己的 `<img>` 本来就是 eager，提交后逐个显示即可；预热只是
           * 给下一批页码提前拉图，不该挡在提交前面。
           */
          decodedPagesRef.current.add(targetPage);
          commitPage(result, targetPage);
          void preloadCoverImages(result.items);
          prefetchAround(targetPage, result.total);
        } else {
          setGroupCounts(
            result.groups
              ? Object.fromEntries(
                  result.groups.map((group) => [group.bucket, group.count]),
                )
              : null,
          );
          setItems((previous) => {
            const source = [
              ...previous,
              ...result.items.filter(
                (item) => !previous.some((entry) => entry.id === item.id),
              ),
            ];
            const seen = new Set<number>();
            return source.filter((item) => {
              if (seen.has(item.id)) return false;
              seen.add(item.id);
              return true;
            });
          });
          setTotal(result.total);
          setPage(targetPage);
        }
      } catch (error) {
        if (requestId === requestIdRef.current) {
          const message = String(error);
          setListError(message);
          notify(message, "error");
        }
      } finally {
        if (requestId === requestIdRef.current) {
          loadingRef.current = false;
          loadingMoreRef.current = false;
          setLoading(false);
          setLoadingMore(false);
        }
      }
    },
    [commitPage, notify, prefetchAround, queryPage],
  );

  useEffect(() => {
    queryTokenRef.current += 1;
    pageCacheRef.current.clear();
    decodedPagesRef.current.clear();
    prefetchPagesRef.current.clear();
    setWarmCovers([]);
  }, [queryPage]);

  useEffect(() => {
    void loadPage(1, true);
  }, [loadPage, viewMode]);

  const runScan = useCallback(
    async (silent: boolean) => {
      setScanning(true);
      try {
        const report = await api.scanLibrary();
        await onLibraryChanged();
        await loadCollections();
        await loadPage(1, true, true);

        const shouldNotify =
          !silent ||
          report.added > 0 ||
          report.updated > 0 ||
          report.missing > 0 ||
          report.errors.length > 0;

        if (shouldNotify) {
          notify(
            report.message,
            report.errors.length > 0 ? "error" : "success",
          );
        }
      } catch (error) {
        notify(`扫描失败：${String(error)}`, "error");
      } finally {
        setScanning(false);
      }
    },
    [loadCollections, loadPage, notify, onLibraryChanged],
  );

  useEffect(() => {
    if (initialScanStartedRef.current) return;
    initialScanStartedRef.current = true;
    void runScan(true);
  }, [runScan]);

  useEffect(() => {
    const unlistenPromise = listen<ImportProgress>(
      "import-progress",
      (event) => setImportProgress(event.payload),
    );
    return () => {
      void unlistenPromise.then((unlisten) => unlisten());
    };
  }, []);

  useEffect(() => {
    if (viewMode !== "scroll") return;
    const node = sentinelRef.current;
    if (!node) return;

    const observer = new IntersectionObserver(
      (entries) => {
        if (
          entries[0]?.isIntersecting &&
          !loadingRef.current &&
          !loadingMoreRef.current &&
          items.length < total
        ) {
          void loadPage(page + 1, false);
        }
      },
      { rootMargin: "600px 0px" },
    );

    observer.observe(node);
    return () => observer.disconnect();
  }, [items.length, loadPage, page, total, viewMode]);

  /** 打开导入面板：先让用户核对清单与标签，再决定写不写库。 */
  const openImport = useCallback((paths: string[]) => {
    setImportSeedPaths(paths);
    setImportOpen(true);
  }, []);

  const startImport = useCallback(
    async (payload: ImportSubmit) => {
      if (payload.paths.length === 0) return;

      setImporting(true);
      setImportProgress({
        current: 0,
        total: 0,
        fileName: "",
        imported: 0,
        skipped: 0,
        failed: 0,
        phase: "start",
      });

      try {
        const report = await api.importPaths(payload.paths, {
          addToCollection: payload.collectionId ?? undefined,
          preserveStructure: payload.preserveStructure,
          tags: payload.tags,
          pathTags: payload.pathTags,
        });
        setImportOpen(false);
        setImportSeedPaths([]);
        notify(report.message, report.failed > 0 ? "error" : "success");
        if (report.errors.length > 0) setImportReport(report);
        // 导入面板里勾了「合成合集」：把这次真正入库的画集按顺序合成。
        const importedIds = report.archiveIds ?? [];
        if (payload.seriesName && importedIds.length >= 2) {
          try {
            await api.createSeries(payload.seriesName, importedIds);
            notify(
              `已合成合集「${payload.seriesName}」（${importedIds.length} 本）`,
              "success",
            );
          } catch (error) {
            notify(`导入成功，但合成合集失败：${String(error)}`, "error");
          }
        }
        await onLibraryChanged();
        await loadCollections();
        await loadTags();
        await loadPage(1, true, true);
      } catch (error) {
        notify(`导入失败：${String(error)}`, "error");
      } finally {
        setImporting(false);
        setImportProgress(null);
      }
    },
    [loadCollections, loadPage, loadTags, notify, onLibraryChanged],
  );

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;

    void getCurrentWebview()
      .onDragDropEvent((event) => {
        const payload = event.payload;
        // 散图整页开着时，拖进来的是图片，由 LoosePanel 自己接（复制进散图区）。
        if (looseView) return;
        if (payload.type === "enter") {
          setDragActive(payload.paths.length > 0);
          return;
        }
        if (payload.type === "leave") {
          setDragActive(false);
          return;
        }
        if (payload.type !== "drop") return;

        setDragActive(false);
        const paths = payload.paths.filter(isSupportedDropPath);
        if (paths.length === 0) {
          notify("请拖入 ZIP / CBZ 压缩包，或包含压缩包的文件夹", "error");
          return;
        }
        if (importing) {
          notify("正在导入，请等当前任务完成", "info");
          return;
        }
        // 面板已经开着就直接并入清单，不再弹第二层面板。
        if (importDialogRef.current) {
          importDialogRef.current.addPaths(paths);
          return;
        }
        openImport(paths);
      })
      .then((stop) => {
        if (disposed) stop();
        else unlisten = stop;
      });

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [importing, notify, openImport, looseView]);

  const pickFiles = useCallback(async () => {
    try {
      const selectedPaths = await open({
        title: "选择压缩包",
        multiple: true,
        directory: false,
        filters: [{ name: "ZIP / CBZ", extensions: ["zip", "cbz"] }],
      });
      if (!selectedPaths) return;
      const paths = Array.isArray(selectedPaths)
        ? selectedPaths
        : [selectedPaths];
      openImport(paths);
    } catch (error) {
      notify(String(error), "error");
    }
  }, [notify, openImport]);

  const pickFolder = useCallback(async () => {
    try {
      const selectedFolder = await open({
        title: "选择包含压缩包的文件夹",
        directory: true,
        multiple: false,
      });
      if (typeof selectedFolder === "string") {
        openImport([selectedFolder]);
      }
    } catch (error) {
      notify(String(error), "error");
    }
  }, [notify, openImport]);

  const submitCollection = useCallback(
    async (name: string) => {
      const trimmed = name.trim();
      if (!trimmed || savingCollection) return;

      setSavingCollection(true);
      try {
        const created = await api.createCollection(trimmed);
        await loadCollections();
        setCollectionCreateIntent(null);
        if (collectionCreateIntent === "batch") {
          setBatchCollectionId(created.id);
          notify(`已新建收藏夹「${created.name}」，点击“加入收藏夹”即可加入所选画集`, "success");
        } else {
          setActiveCollectionId(created.id);
          notify(`已新建收藏夹「${created.name}」`, "success");
        }
      } catch (error) {
        notify(`新建收藏夹失败：${String(error)}`, "error");
      } finally {
        setSavingCollection(false);
      }
    },
    [collectionCreateIntent, loadCollections, notify, savingCollection],
  );

  const toggleCollectionMembership = useCallback(
    async (archiveId: number, collectionId: number, member: boolean) => {
      await api.setArchiveCollection(archiveId, collectionId, member);
      await loadCollections();
      if (
        (activeCollectionId === collectionId && !member) ||
        groupBy === "collection"
      ) {
        await loadPage(1, true, true);
      }
    },
    [activeCollectionId, groupBy, loadCollections, loadPage],
  );

  const setCollectionCover = useCallback(
    async (collectionId: number, archiveId: number | null) => {
      await api.setCollectionCover(collectionId, archiveId);
      await loadCollections();
    },
    [loadCollections],
  );

  const refreshCollectionsAfterEdit = useCallback(async () => {
    await loadCollections();
    if (activeCollectionId !== null || groupBy === "collection") {
      await loadPage(1, true, true);
    }
  }, [activeCollectionId, groupBy, loadCollections, loadPage]);

  /**
   * 阅读器里直接切上一本 / 下一本。
   * 浏览历史只记录集合级状态（快照里没有 reader），所以这里换书
   * 不会压历史——后退永远是「退出阅读器」，不会回放上一本。
   */
  /** 连读跨页取书在途标记：连按 ]/[ 时避免重复发起同一页查询。 */
  const switchingReaderRef = useRef(false);

  /** 当前正在读的本子在列表里的位置；-1 表示不在当前列表里。 */
  const readerIndex = useMemo(
    () =>
      readerCard
        ? items.findIndex((item) => item.id === readerCard.id)
        : -1,
    [items, readerCard],
  );

  /** 合集连读：当前本子所属合集的成员顺序；不在合集里（或本子不在其中）时为 null。 */
  const readerSeriesNav = useMemo(() => {
    if (!readerCard || !readerSeriesIds) return null;
    if (!readerSeriesIds.includes(readerCard.id)) return null;
    const index = readerSeriesIds.indexOf(readerCard.id);
    return {
      prev: index > 0 ? readerSeriesIds[index - 1] : null,
      next:
        index < readerSeriesIds.length - 1 ? readerSeriesIds[index + 1] : null,
    };
  }, [readerCard, readerSeriesIds]);

  const switchReaderArchive = useCallback(
    (direction: -1 | 1) => {
      if (!readerCard) return;
      // 合集连读：按合集的集数顺序切，不按画廊网格顺序。
      if (readerSeriesNav) {
        const targetId =
          direction === 1 ? readerSeriesNav.next : readerSeriesNav.prev;
        if (targetId == null) return;
        if (switchingReaderRef.current) return;
        switchingReaderRef.current = true;
        void (async () => {
          try {
            const result = await api.queryArchives({
              archiveIds: [targetId],
              pageSize: 12,
            });
            const card = result.items[0];
            if (card) setReaderCard(card);
          } catch {
            // 取不到就停在当前话，不打断阅读
          } finally {
            switchingReaderRef.current = false;
          }
        })();
        return;
      }
      const index = items.findIndex((item) => item.id === readerCard.id);
      if (index < 0) return;
      const next = items[index + direction];
      if (next) {
        setReaderCard(next);
        return;
      }
      // 页边界：直接向库查相邻页的端点本子。不动 items/页码——画廊列表保持
      // 原样，只有阅读器快照换书；连点用 switchingReaderRef 去重。
      if (switchingReaderRef.current) return;
      const targetPage = direction === 1 ? page + 1 : page - 1;
      if (targetPage < 1 || targetPage > totalPagesRef.current) return;
      switchingReaderRef.current = true;
      void (async () => {
        try {
          const result = await queryPage(targetPage);
          const adjacent =
            direction === 1
              ? result.items[0]
              : result.items[result.items.length - 1];
          if (adjacent) setReaderCard(adjacent);
        } catch {
          // 取不到相邻页就停在当前本子，不打断阅读
        } finally {
          switchingReaderRef.current = false;
        }
      })();
    },
    [readerSeriesNav, items, page, queryPage, readerCard],
  );

  const updateReaderProgress = useCallback(
    (id: number, lastPage: number) => {
      const lastReadAt = Date.now();
      const patch = (item: ArchiveCardData) =>
        item.id === id ? { ...item, lastPage, lastReadAt } : item;
      patchCachedCards(patch);
      setItems((previous) => previous.map(patch));
      setSelected((current) => (current ? patch(current) : current));
      setReaderCard((current) => (current ? patch(current) : current));
    },
    [patchCachedCards],
  );

  const updateNote = useCallback(
    (id: number, note: string | null) => {
      const patch = (item: ArchiveCardData) =>
        item.id === id ? { ...item, note } : item;
      patchCachedCards(patch);
      setItems((previous) => previous.map(patch));
      setSelected((current) =>
        current && current.id === id ? { ...current, note } : current,
      );
    },
    [patchCachedCards],
  );

  const toggleSelect = useCallback((id: number) => {
    setSelectedIds((previous) =>
      previous.includes(id)
        ? previous.filter((item) => item !== id)
        : [...previous, id],
    );
  }, []);

  const exitSelection = useCallback(() => {
    setSelectionMode(false);
    setSelectedIds([]);
    setBatchCollectionId("");
    setBatchQueueId("");
    setBatchDeleteConfirm(false);
  }, []);

  /** 把当前多选的画集合成一个合集：主网格从此只留一张轮换封面的卡。 */
  const createSeriesFromSelection = useCallback(
    async (name: string) => {
      if (selectedIds.length < 2) return;
      setSeriesBusy(true);
      try {
        await api.createSeries(name, selectedIds);
        notify(`已合成合集「${name}」`, "success");
        setSeriesCreateOpen(false);
        exitSelection();
        setSeriesList(await api.listSeries());
        void loadPage(1, true, true);
      } catch (error) {
        notify(`合成合集失败：${String(error)}`, "error");
      } finally {
        setSeriesBusy(false);
      }
    },
    [selectedIds, notify, exitSelection, loadPage],
  );

  // 多选模式一开就把待看目录拉过来：批量「加入待看」的下拉要用。
  const refreshQueues = useCallback(async () => {
    try {
      setQueues(await api.listQueues());
    } catch {
      // 拉不到就保持空列表：下拉为空时按钮会提示先建目录
    }
  }, []);

  useEffect(() => {
    if (selectionMode) void refreshQueues();
  }, [selectionMode, refreshQueues]);

  const refreshAfterTrashChange = useCallback(async () => {
    await onLibraryChanged();
    await loadCollections();
    await loadTags();
    await loadPage(1, true, true);
  }, [loadCollections, loadPage, loadTags, onLibraryChanged]);

  const removeArchive = useCallback(
    async (id: number) => {
      await api.deleteArchive(id);
      setSelected(null);
      setReaderCard(null);
      await refreshAfterTrashChange();
      notify("已移入回收站，可在侧栏「回收站」恢复", "success");
    },
    [notify, refreshAfterTrashChange],
  );

  const deleteSelected = useCallback(async () => {
    if (batchBusy || selectedIds.length === 0) return;
    if (!batchDeleteConfirm) {
      setBatchDeleteConfirm(true);
      window.setTimeout(() => setBatchDeleteConfirm(false), 3200);
      return;
    }

    setBatchDeleteConfirm(false);
    setBatchBusy(true);
    try {
      await Promise.all(selectedIds.map((id) => api.deleteArchive(id)));
      const count = selectedIds.length;
      exitSelection();
      await refreshAfterTrashChange();
      notify(`已把 ${count} 个画集移入回收站`, "success");
    } catch (error) {
      notify(`移入回收站失败：${String(error)}`, "error");
    } finally {
      setBatchBusy(false);
    }
  }, [
    batchBusy,
    batchDeleteConfirm,
    exitSelection,
    notify,
    refreshAfterTrashChange,
    selectedIds,
  ]);

  const addSelectedToCollection = useCallback(async () => {
    if (batchBusy) return;
    const collectionId = Number(batchCollectionId);
    if (!collectionId || selectedIds.length === 0) {
      notify("请先选择收藏夹", "error");
      return;
    }

    setBatchBusy(true);
    try {
      const added = await api.addArchivesToCollection(
        collectionId,
        selectedIds,
      );
      await loadCollections();
      if (activeCollectionId === collectionId || groupBy === "collection") {
        await loadPage(1, true, true);
      }
      notify(`已把 ${added} 个画集加入收藏夹`, "success");
      exitSelection();
    } catch (error) {
      notify(`批量加入失败：${String(error)}`, "error");
    } finally {
      setBatchBusy(false);
    }
  }, [
    activeCollectionId,
    batchBusy,
    batchCollectionId,
    exitSelection,
    loadCollections,
    loadPage,
    notify,
    selectedIds,
  ]);

  /** 多选的画集按选择顺序追加进待看目录（任意数量；重复的本自动跳过）。 */
  const addSelectedToQueue = useCallback(async () => {
    if (batchBusy || selectedIds.length === 0) return;
    const queueId = Number(batchQueueId);
    if (!queueId) {
      notify("请先选择待看目录（没有就先去「待看目录」面板新建）", "error");
      return;
    }

    setBatchBusy(true);
    try {
      const added = await api.addManyToQueue(queueId, selectedIds);
      const target = queues.find((queue) => queue.id === queueId);
      await refreshQueues();
      notify(
        added > 0
          ? `已把 ${added} 个画集加入「${target?.name ?? "待看目录"}」`
          : "选中的画集都已经在目录里了",
        added > 0 ? "success" : "info",
      );
      exitSelection();
    } catch (error) {
      notify(`批量加入待看失败：${String(error)}`, "error");
    } finally {
      setBatchBusy(false);
    }
  }, [
    batchBusy,
    batchQueueId,
    exitSelection,
    notify,
    queues,
    refreshQueues,
    selectedIds,
  ]);

  /** 队列视图里把一本往前/往后挪一格；挪完刷新当前页（顺序即视图顺序）。 */
  const moveQueueCard = useCallback(
    async (archiveId: number, direction: -1 | 1) => {
      if (!activeQueue) return;
      try {
        await api.moveQueueArchive(activeQueue.id, archiveId, direction);
        await loadPage(1, true, true);
      } catch (error) {
        notify(`调整顺序失败：${String(error)}`, "error");
      }
    },
    [activeQueue, loadPage, notify],
  );

  // —— 卡片右键菜单 ——
  const openCardMenu = useCallback((card: ArchiveCardData, x: number, y: number) => {
    // 视口内收放：菜单大约 210×240
    const menuX = Math.min(x, window.innerWidth - 224);
    const menuY = Math.min(y, window.innerHeight - 252);
    setCardMenu({ card, x: Math.max(4, menuX), y: Math.max(4, menuY) });
  }, []);
  const closeCardMenu = useCallback(() => setCardMenu(null), []);

  // 菜单开着时：Esc 关、滚动关、点空白关（backdrop 自带 onClick）
  useEffect(() => {
    if (!cardMenu) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setCardMenu(null);
    };
    window.addEventListener("keydown", onKey);
    window.addEventListener("scroll", () => setCardMenu(null), true);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("scroll", () => setCardMenu(null), true);
    };
  }, [cardMenu]);

  /** 右键「加入待看」：加入最新创建的目录（没有就引导去面板新建）。 */
  const addCardToQueue = useCallback(async (card: ArchiveCardData) => {
    const list = await api.listQueues();
    setQueues(list);
    const newest = list[0];
    if (!newest) {
      notify("还没有待看目录——先去「待看目录」面板「新建空目录」", "error");
      return;
    }
    const added = await api.addManyToQueue(newest.id, [card.id]);
    notify(
      added > 0
        ? `已把「${card.title}」加入「${newest.name}」`
        : "这本已经在目录里了",
      added > 0 ? "success" : "info",
    );
  }, [notify]);

  /** 右键「移入回收站」：可恢复操作，不做二次确认。 */
  const trashFromMenu = useCallback(async (card: ArchiveCardData) => {
    try {
      await api.deleteArchive(card.id);
      await refreshAfterTrashChange();
      notify(`「${card.title}」已移入回收站，可在侧栏「回收站」恢复`, "success");
    } catch (error) {
      notify(`移入回收站失败：${String(error)}`, "error");
    }
  }, [notify, refreshAfterTrashChange]);

  const startRenameCollection = useCallback((collection: Collection) => {
    setRenamingCollectionId(collection.id);
    setRenameDraft(collection.name);
    setPendingDeleteCollectionId(null);
  }, []);

  const submitRenameCollection = useCallback(async () => {
    const name = renameDraft.trim();
    if (!name || renameBusy || renamingCollectionId === null) return;
    setRenameBusy(true);
    try {
      await api.renameCollection(renamingCollectionId, name);
      setCollections(await api.listCollections());
      setRenamingCollectionId(null);
      setRenameDraft("");
      notify("收藏夹已重命名", "success");
    } catch (error) {
      notify(`重命名失败：${String(error)}`, "error");
    } finally {
      setRenameBusy(false);
    }
  }, [notify, renameBusy, renameDraft, renamingCollectionId]);

  const removeCollection = useCallback(
    async (collection: Collection) => {
      if (pendingDeleteCollectionId !== collection.id) {
        setPendingDeleteCollectionId(collection.id);
        window.setTimeout(() => {
          setPendingDeleteCollectionId((current) =>
            current === collection.id ? null : current,
          );
        }, 3200);
        return;
      }

      setPendingDeleteCollectionId(null);
      try {
        await api.deleteCollection(collection.id);
        if (activeCollectionId === collection.id) setActiveCollectionId(null);
        setCollections(await api.listCollections());
        notify(`已把收藏夹「${collection.name}」移入回收站`, "success");
      } catch (error) {
        notify(`删除收藏夹失败：${String(error)}`, "error");
      }
    },
    [activeCollectionId, notify, pendingDeleteCollectionId],
  );

  const moveCollection = useCallback(
    async (collectionId: number, delta: number) => {
      const index = collections.findIndex((item) => item.id === collectionId);
      const target = index + delta;
      if (index < 0 || target < 1 || target >= collections.length) return;

      const next = [...collections];
      const [moved] = next.splice(index, 1);
      next.splice(target, 0, moved);
      setCollections(next);

      try {
        await api.reorderCollections(next.map((item) => item.id));
      } catch (error) {
        setCollections(await api.listCollections());
        notify(`调整排序失败：${String(error)}`, "error");
      }
    },
    [collections, notify],
  );

  useEffect(() => {
    setSelectedIds([]);
  }, [
    activeCollectionId,
    activeTagIds,
    searchTerm,
    tagMode,
    ratingBuckets,
    readStatuses,
    missingOnly,
  ]);

  const activeCollection =
    collections.find((collection) => collection.id === activeCollectionId) ??
    null;
  const activeTags = tags.filter((tag) => activeTagIds.includes(tag.id));
  const filteredTagOptions = tags.filter((tag) => {
    const keyword = tagQuery.trim().toLowerCase();
    if (!keyword) return true;
    return (
      tag.name.toLowerCase().includes(keyword) ||
      (tag.translatedName?.toLowerCase().includes(keyword) ?? false)
    );
  });
  const hasTagFilter = activeTagIds.length > 0;
  const hasRatingFilter = ratingBuckets.length > 0;
  const hasReadFilter = readStatuses.length > 0;
  const hasAdvancedFilter = hasRatingFilter || hasReadFilter || missingOnly;
  const hasFilter = hasTagFilter || hasAdvancedFilter;
  const filterCount =
    ratingBuckets.length + readStatuses.length + (missingOnly ? 1 : 0);
  const activeRatingBuckets = RATING_FILTER_OPTIONS.filter((option) =>
    ratingBuckets.includes(option.bucket),
  );
  const activeReadStatuses = READ_STATUS_OPTIONS.filter((option) =>
    readStatuses.includes(option.value),
  );

  const toggleRatingBucket = useCallback((bucket: number) => {
    setRatingBuckets((previous) =>
      previous.includes(bucket)
        ? previous.filter((value) => value !== bucket)
        : [...previous, bucket],
    );
  }, []);

  const toggleReadStatus = useCallback((status: ReadStatus) => {
    setReadStatuses((previous) =>
      previous.includes(status)
        ? previous.filter((value) => value !== status)
        : [...previous, status],
    );
  }, []);

  const clearAdvancedFilters = useCallback(() => {
    setRatingBuckets([]);
    setReadStatuses([]);
    setMissingOnly(false);
  }, []);

  const clearAllFilters = useCallback(() => {
    setActiveTagIds([]);
    clearAdvancedFilters();
  }, [clearAdvancedFilters]);

  const toggleTagFilter = useCallback(
    (tagId: number) => {
      if (!activeTagIds.includes(tagId)) {
        setActiveCollectionId(null);
      }
      setActiveTagIds((previous) =>
        previous.includes(tagId)
          ? previous.filter((id) => id !== tagId)
          : [...previous, tagId],
      );
    },
    [activeTagIds],
  );

  const browseTag = useCallback((tagId: number) => {
    setTagCenterOpen(false);
    setTagFilterOpen(false);
    setActiveCollectionId(null);
    setActiveTagIds([tagId]);
  }, []);

  const totalPages = Math.max(1, Math.ceil(total / pageSize));
  const hasMore = items.length < total;

  const groupedItems = useMemo(() => {
    if (groupBy === "none") {
      return [{ bucket: -1, items }];
    }
    const groups: { bucket: number; items: ArchiveCardData[] }[] = [];
    for (const item of items) {
      const bucket =
        groupBy === "rating" ? ratingBucket(item.rating) : item.groupBucket ?? 0;
      const last = groups[groups.length - 1];
      if (last && last.bucket === bucket) {
        last.items.push(item);
      } else {
        groups.push({ bucket, items: [item] });
      }
    }
    return groups;
  }, [groupBy, items]);

  // 队列视图里给卡片挂 ↑↓：首尾两端对应方向禁用（分组时取扁平序）。
  const queueEdgeIds = useMemo(() => {
    if (!activeQueue) return null;
    const ids = groupedItems.flatMap((group) =>
      group.items.map((card) => card.id),
    );
    return { first: ids[0], last: ids[ids.length - 1] };
  }, [activeQueue, groupedItems]);

  const overlayOpen =
    selected !== null ||
    readerCard !== null ||
    quickEditCard !== null ||
    tagCenterOpen ||
    trashOpen ||
    tagFilterOpen ||
    filterOpen ||
    selectionMode ||
    importing ||
    importReport !== null ||
    dragActive;

  const goToPage = useCallback(
    (target: number) => {
      if (viewModeRef.current !== "pages" || loadingRef.current) return;
      const next = Math.min(
        Math.max(1, target),
        Math.max(1, totalPagesRef.current),
      );
      if (next === pageRef.current) return;
      void loadPage(next, true);
    },
    [loadPage],
  );

  useLayoutEffect(() => {
    if (viewMode !== "pages") return;
    contentRef.current?.scrollTo({ top: 0 });
  }, [page, viewMode]);

  useEffect(() => {
    if (viewMode !== "pages") return;
    const handleKeyDown = (event: KeyboardEvent) => {
      if (overlayOpen || event.defaultPrevented) return;
      if (event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) {
        return;
      }
      const target = event.target instanceof HTMLElement ? event.target : null;
      if (
        target &&
        (target.tagName === "INPUT" ||
          target.tagName === "TEXTAREA" ||
          target.tagName === "SELECT" ||
          target.isContentEditable ||
          target.closest("[role='dialog']"))
      ) {
        return;
      }

      let step = 0;
      if (event.key === "ArrowLeft" || event.key === "ArrowUp") {
        step = -1;
      } else if (event.key === "ArrowRight" || event.key === "ArrowDown") {
        step = 1;
      } else {
        return;
      }

      event.preventDefault();
      goToPage(pageRef.current + step);
    };

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [goToPage, overlayOpen, viewMode]);

  useEffect(() => {
    const node = contentRef.current;
    if (!node) return;

    let accumulated = 0;
    let lastWheelAt = 0;
    let lastFlipAt = 0;

    const handleWheel = (event: WheelEvent) => {
      if (viewModeRef.current !== "pages" || totalPagesRef.current <= 1) return;
      if (event.ctrlKey || event.metaKey) return;
      if (
        event.target instanceof Element &&
        event.target.closest(
          "[role='dialog'], .filter-popover, .tag-filter-popover, .name-dialog",
        )
      ) {
        return;
      }

      const unit =
        event.deltaMode === 1
          ? 16
          : event.deltaMode === 2
            ? node.clientHeight
            : 1;
      const dx = event.deltaX * unit;
      const dy = event.deltaY * unit;
      const horizontal = event.shiftKey || Math.abs(dx) > Math.abs(dy);
      const delta = horizontal
        ? event.shiftKey && dx === 0
          ? dy
          : dx
        : dy;
      if (delta === 0) return;

      if (!horizontal) {
        const atTop = node.scrollTop <= 0;
        const atBottom =
          node.scrollTop + node.clientHeight >= node.scrollHeight - 2;
        if (!(delta < 0 && atTop) && !(delta > 0 && atBottom)) {
          accumulated = 0;
          return;
        }
      }

      event.preventDefault();

      const now = performance.now();
      if (now - lastWheelAt > 200) accumulated = 0;
      lastWheelAt = now;
      if (now - lastFlipAt < 480) {
        accumulated = 0;
        return;
      }
      accumulated += delta;
      if (Math.abs(accumulated) < 32) return;

      const step = accumulated > 0 ? 1 : -1;
      accumulated = 0;
      lastFlipAt = now;
      goToPage(pageRef.current + step);
    };

    const shell = node.parentElement ?? node;
    shell.addEventListener("wheel", handleWheel, { passive: false });
    return () => shell.removeEventListener("wheel", handleWheel);
  }, [goToPage]);

  useEffect(() => {
    if (viewMode !== "pages" || totalPages <= 1) setAutoPaging(false);
  }, [totalPages, viewMode]);

  useEffect(() => {
    if (!autoPaging) return;
    if (viewMode !== "pages" || totalPages <= 1 || overlayOpen) return;

    const timer = window.setInterval(() => {
      if (loadingRef.current) return;
      const current = pageRef.current;
      const next = current >= totalPagesRef.current ? 1 : current + 1;
      void loadPage(next, true);
    }, autoPageSeconds * 1000);

    return () => window.clearInterval(timer);
  }, [
    autoPageSeconds,
    autoPaging,
    loadPage,
    overlayOpen,
    page,
    totalPages,
    viewMode,
  ]);

  return (
    <div
      className="app-shell"
      style={{
        gridTemplateColumns: `${
          sidebarCollapsed ? SIDEBAR_RAIL_WIDTH : sidebarWidth
        }px 6px minmax(0, 1fr)`,
      }}
    >
      <aside
        className={`sidebar${sidebarCollapsed ? " is-collapsed" : ""}${
          !sidebarCollapsed && sidebarWidth < 224 ? " is-tight" : ""
        }${!sidebarCollapsed && sidebarWidth < 150 ? " is-compact" : ""}`}
      >
        <div className="sidebar-brand">
          <span className="brand-mark">
            <LogoIcon />
          </span>
          <div>
            <strong>画匣</strong>
            <span>本地画廊</span>
          </div>
          <button
            type="button"
            className="sidebar-collapse"
            onClick={toggleSidebarCollapsed}
            aria-label={sidebarCollapsed ? "展开侧栏" : "收起侧栏"}
            aria-expanded={!sidebarCollapsed}
            title={sidebarCollapsed ? "展开侧栏（双击分界线也行）" : "收起成图标条（双击分界线也行）"}
          >
            <svg
              viewBox="0 0 16 16"
              width={14}
              height={14}
              fill="none"
              stroke="currentColor"
              strokeWidth={1.5}
              strokeLinecap="round"
              strokeLinejoin="round"
              aria-hidden="true"
              className={sidebarCollapsed ? "is-flipped" : undefined}
            >
              <path d="M9.5 4.5 6 8l3.5 3.5" />
            </svg>
          </button>
        </div>

        <section className="library-card">
          <p className="eyebrow">CURRENT LIBRARY</p>
          <h2 title={library.libraryName ?? ""}>
            {library.libraryName ?? "未命名库"}
          </h2>
          <p className="library-path" title={library.libraryPath ?? ""}>
            {library.libraryPath}
          </p>
          <div className="library-stats">
            <span>{library.archiveCount} 个画集</span>
            <span>{formatBytes(library.totalSize)}</span>
          </div>
          {library.missingCount > 0 && (
            <div className="missing-chip">
              <AlertIcon width={12} height={12} />
              {library.missingCount} 个文件缺失
            </div>
          )}
        </section>

        <nav className="sidebar-nav" aria-label="画廊导航">
          <button
            type="button"
            className={`nav-item${
              activeCollectionId === null && !activeQueue ? " is-active" : ""
            }`}
            onClick={() => {
              setLooseView(false);
              setActiveCollectionId(null);
              clearActiveQueue();
            }}
            title="全部画集"
          >
            <BookIcon />
            <span className="nav-label">全部画集</span>
            <span className="nav-count">{library.archiveCount}</span>
          </button>

          <button
            type="button"
            className="nav-item nav-group-toggle"
            onClick={toggleCollectionsCollapsed}
            aria-expanded={!collectionsCollapsed}
            title={collectionsCollapsed ? "展开收藏夹列表" : "折叠收藏夹列表"}
          >
            <span className={`nav-caret${collectionsCollapsed ? "" : " is-open"}`}>▸</span>
            <span className="nav-label">收藏夹</span>
            <span className="nav-count">{collections.length}</span>
          </button>

          {!collectionsCollapsed &&
            collections.map((collection, index) => {
            const isRenaming = renamingCollectionId === collection.id;
            if (isRenaming) {
              return (
                <form
                  key={collection.id}
                  className="nav-create"
                  onSubmit={(event) => {
                    event.preventDefault();
                    void submitRenameCollection();
                  }}
                >
                  <input
                    autoFocus
                    value={renameDraft}
                    disabled={renameBusy}
                    placeholder="新的收藏夹名称"
                    onChange={(event) => setRenameDraft(event.target.value)}
                    onKeyDown={(event) => {
                      if (event.key === "Escape") {
                        setRenamingCollectionId(null);
                        setRenameDraft("");
                      }
                    }}
                  />
                  <button
                    type="submit"
                    className="btn btn-primary btn-compact"
                    disabled={renameBusy || !renameDraft.trim()}
                  >
                    保存
                  </button>
                  <button
                    type="button"
                    className="btn btn-quiet btn-compact"
                    onClick={() => {
                      setRenamingCollectionId(null);
                      setRenameDraft("");
                    }}
                  >
                    取消
                  </button>
                </form>
              );
            }

            const isActive = activeCollectionId === collection.id;
            // 默认收藏夹在界面上统一叫「默认收藏」（库里的名字不动，避免动用户数据）。
            const displayName = collection.isDefault ? "默认收藏" : collection.name;
            const confirmingDelete =
              pendingDeleteCollectionId === collection.id;
            return (
              <div
                className={`nav-row${isActive ? " is-active" : ""}`}
                key={collection.id}
              >
                <button
                  type="button"
                  className={`nav-item${isActive ? " is-active" : ""}`}
                  title={displayName}
                  onClick={() => {
                    setLooseView(false);
                    setActiveCollectionId(collection.id);
                  }}
                >
                  <CollectionGlyph
                    icon={collection.icon}
                    color={collection.color}
                  />
                  <span className="nav-label">{displayName}</span>
                  <span className="nav-count">{collection.count}</span>
                </button>
                <div className="nav-row-actions">
                  <button
                    type="button"
                    className="nav-mini"
                    onClick={(event) => {
                      event.stopPropagation();
                      setStyleEditing({
                        id: collection.id,
                        x: event.clientX,
                        y: event.clientY,
                      });
                    }}
                    title="自定义图标与颜色"
                    aria-label={`自定义 ${collection.name} 的图标`}
                  >
                    <CollectionGlyph
                      icon={collection.icon}
                      color={collection.color}
                      size={12}
                    />
                  </button>
                  <button
                    type="button"
                    className="nav-mini"
                    disabled={index <= 1}
                    onClick={() => void moveCollection(collection.id, -1)}
                    title="上移收藏夹"
                    aria-label={`上移 ${collection.name}`}
                  >
                    ↑
                  </button>
                  <button
                    type="button"
                    className="nav-mini"
                    disabled={index >= collections.length - 1}
                    onClick={() => void moveCollection(collection.id, 1)}
                    title="下移收藏夹"
                    aria-label={`下移 ${collection.name}`}
                  >
                    ↓
                  </button>
                  <button
                    type="button"
                    className="nav-mini"
                    onClick={() => startRenameCollection(collection)}
                    title="重命名收藏夹"
                    aria-label={`重命名 ${collection.name}`}
                  >
                    改
                  </button>
                  <button
                    type="button"
                    className={`nav-mini${confirmingDelete ? " is-danger" : ""}`}
                    disabled={collection.isDefault}
                    onClick={() => void removeCollection(collection)}
                    title={
                      collection.isDefault
                        ? "默认收藏夹不可删除"
                        : confirmingDelete
                          ? "再次点击确认删除"
                          : "删除收藏夹"
                    }
                    aria-label={`删除 ${collection.name}`}
                  >
                    {confirmingDelete ? "确认" : "删"}
                  </button>
                </div>
              </div>
            );
          })}

          <div className="nav-divider" aria-hidden="true" />

          <button
            type="button"
            className="nav-item nav-item-tags"
            onClick={() => {
              setLooseView(false);
              setTagCenterOpen(true);
            }}
            title="管理标签、搜索标签、生成标签导图"
          >
            <TagIcon width={16} height={16} />
            <span className="nav-label">标签中心</span>
            <span className="nav-count">{tags.length}</span>
          </button>

          <button
            type="button"
            className="nav-item"
            onClick={() => {
              setLooseView(false);
              setQueuePanelOpen(true);
            }}
            title="随机挑一批画集排队看，带进度"
          >
            <BookIcon width={16} height={16} />
            <span className="nav-label">待看目录</span>
          </button>

          <button
            type="button"
            className={`nav-item${looseView ? " is-active" : ""}`}
            onClick={() => setLooseView((open) => !open)}
            title="散图收藏夹：单图多区收藏，文件夹形式只记引用"
          >
            <FolderIcon width={16} height={16} />
            <span className="nav-label">散图收藏夹</span>
          </button>

          <div className="nav-divider" aria-hidden="true" />

          <button
            type="button"
            className="nav-item"
            onClick={() => setReleasePanelOpen(true)}
            title="检查更新并立即安装"
          >
            <RefreshIcon width={16} height={16} />
            <span className="nav-label">更新画匣</span>
          </button>

          <button
            type="button"
            className="nav-item"
            onClick={() => setSettingsOpen(true)}
            title="统一设置：视图 / 快捷键 / 存储 / 各面板入口"
          >
            <SwapIcon width={16} height={16} />
            <span className="nav-label">设置</span>
          </button>

          <button
            type="button"
            className="nav-item"
            onClick={() => setD2PanelOpen(true)}
            title="D2 下载器最小闭环测试（exhentai 第一页）"
          >
            <span className="nav-label">下载测试</span>
          </button>

          {activeQueue && (
            <div className="nav-row is-active">
              <button
                type="button"
                className="nav-item is-active"
                onClick={() => setQueuePanelOpen(true)}
                title={`${activeQueue.name}：${activeQueue.finished}/${activeQueue.count} 已读完`}
              >
                <BookIcon width={16} height={16} />
                <span className="nav-label">{activeQueue.name}</span>
                <span className="nav-count">
                  {activeQueue.finished}/{activeQueue.count}
                </span>
              </button>
              <div className="nav-row-actions">
                <button
                  type="button"
                  className="nav-mini"
                  onClick={clearActiveQueue}
                  title="退出待看目录"
                  aria-label="退出待看目录"
                >
                  ×
                </button>
              </div>
            </div>
          )}

          <button
            type="button"
            className="nav-item"
            onClick={() => setTrashOpen(true)}
            title="恢复或彻底删除画集、收藏夹与标签"
          >
            <TrashIcon width={16} height={16} />
            <span className="nav-label">回收站</span>
          </button>

          <button
            type="button"
            className="nav-item nav-item-add"
            onClick={() => setCollectionCreateIntent("browse")}
            title="新建收藏夹"
          >
            <PlusIcon width={15} height={15} />
            <span className="nav-label">新建收藏夹</span>
          </button>
        </nav>

        <div className="sidebar-actions">
          <button
            className="btn btn-primary"
            onClick={() => void pickFiles()}
            disabled={importing}
          >
            <ImportIcon />
            导入压缩包
          </button>
          <button
            className="btn btn-ghost"
            onClick={() => void pickFolder()}
            disabled={importing}
          >
            <FolderIcon />
            导入文件夹
          </button>
          <button
            className="btn btn-ghost"
            onClick={() => void runScan(false)}
            disabled={scanning || importing}
          >
            <RefreshIcon />
            {scanning ? "正在同步…" : "重新扫描"}
          </button>
          <button
            className="btn btn-quiet"
            onClick={onSwitchLibrary}
            disabled={importing}
          >
            <SwapIcon />
            切换库目录
          </button>
        </div>

        <div className="sidebar-footer">
          <span className="footer-dot" />
          {appVersion ? `v${appVersion}` : "画匣"} · ZIP/CBZ/7Z/RAR · 完全离线
        </div>
      </aside>

      <Splitter
        label="侧栏"
        value={sidebarWidth}
        min={62}
        max={420}
        onChange={(next) => {
          setSidebarWidth(next);
          if (sidebarCollapsed) setSidebarCollapsed(false);
        }}
        onReset={resetSidebarWidth}
        onToggle={toggleSidebarCollapsed}
        toggleTitle={
          sidebarCollapsed
            ? "双击展开侧栏（拖动也可展开）"
            : "双击收起成图标条（Shift+回车复位宽度）"
        }
      />

      <main
        className="main-panel"
        onContextMenu={(event) => {
          // 卡片自己的右键菜单已经 preventDefault，这里只管空白处。
          if (event.defaultPrevented) return;
          const target = event.target as HTMLElement;
          if (
            target.closest(
              ".archive-card, button, input, select, textarea, a, .modal-backdrop",
            )
          ) {
            return;
          }
          event.preventDefault();
          setBgMenu({ x: event.clientX, y: event.clientY });
        }}
      >
        {looseView && <LoosePanel variant="page" onNotify={notify} />}
        <header className="topbar">
          <div className="topbar-title">
            <p className="eyebrow">
              {searchTerm
                ? "SEARCH"
                : hasAdvancedFilter
                  ? "FILTER"
                  : hasTagFilter
                    ? "TAG FILTER"
                    : activeCollection
                      ? "COLLECTION"
                      : "LIBRARY"}
            </p>
            <h1>
              {searchTerm
                ? `搜索“${searchTerm}”`
                : hasAdvancedFilter
                  ? "筛选结果"
                  : hasTagFilter
                    ? activeTags
                        .map((tag) => `#${tag.translatedName || tag.name}`)
                        .join(" ")
                    : (activeCollection
                        ? activeCollection.isDefault
                          ? "默认收藏"
                          : activeCollection.name
                        : "全部画集")}
            </h1>
            <span className="title-meta">
              {total} 个画集
              {groupBy === "rating"
                ? " · 按评分分组"
                : groupBy === "tag"
                  ? " · 按标签分组"
                  : groupBy === "collection"
                    ? " · 按收藏夹分组"
                    : ""}
              {activeCollection || hasFilter || searchTerm
                ? ""
                : ` · ${formatBytes(library.totalSize)}`}
            </span>
            {activeCollection && (
              <div className="collection-page-actions">
                <button
                  type="button"
                  className="btn btn-ghost btn-compact"
                  onClick={() => setCollectionCreateIntent("browse")}
                  title="新建收藏夹并自定义名称"
                >
                  <PlusIcon width={13} height={13} />
                  新建收藏夹
                </button>
              </div>
            )}
          </div>

          <div className="topbar-tools">
            {scanning && (
              <span className="status-chip">
                <span className="spinner" />
                同步中
              </span>
            )}

            <div className="history-nav" role="group" aria-label="浏览历史">
              <button
                type="button"
                className="icon-button"
                onClick={goBack}
                disabled={!canGoBack}
                title="后退（鼠标侧键）"
                aria-label="后退"
              >
                <ChevronLeftIcon width={16} height={16} />
              </button>
              <button
                type="button"
                className="icon-button"
                onClick={goForward}
                disabled={!canGoForward}
                title="前进（鼠标侧键）"
                aria-label="前进"
              >
                <ChevronRightIcon width={16} height={16} />
              </button>
            </div>

            <div className="search-field">
              <SearchIcon width={14} height={14} />
              <input
                type="search"
                value={search}
                placeholder="搜索标题 / 文件名 / 备注"
                aria-label="搜索画集"
                onChange={(event) => setSearch(event.target.value)}
              />
              {search && (
                <button
                  type="button"
                  className="search-clear"
                  onClick={() => setSearch("")}
                  aria-label="清空搜索"
                >
                  <CloseIcon width={12} height={12} />
                </button>
              )}
            </div>

            <button
              type="button"
              className={`btn btn-ghost btn-compact${
                selectionMode ? " is-filtering" : ""
              }`}
              onClick={() =>
                selectionMode ? exitSelection() : setSelectionMode(true)
              }
              title="多选画集后批量加入收藏夹"
            >
              <CheckIcon width={14} height={14} />
              {selectionMode ? `已选 ${selectedIds.length}` : "多选"}
            </button>

            <div className="tag-filter-wrap" ref={tagFilterRef}>
              <button
                type="button"
                className={`btn btn-ghost btn-compact${
                  hasTagFilter ? " is-filtering" : ""
                }`}
                onClick={() => {
                  setFilterOpen(false);
                  setTagFilterOpen((current) => !current);
                }}
                title="标签筛选：AND 同时含 / OR 任一 / 排除；不选标签则不筛选"
              >
                <TagIcon width={15} height={15} />
                标签
                {hasTagFilter && (
                  <span className="filter-count">{activeTagIds.length}</span>
                )}
              </button>

              {tagFilterOpen && (
                <div className="tag-filter-popover">
                  <div className="tag-filter-search">
                    <SearchIcon width={14} height={14} />
                    <input
                      autoFocus
                      value={tagQuery}
                      placeholder="搜索标签或中文译名"
                      onChange={(event) => setTagQuery(event.target.value)}
                    />
                  </div>
                  <div className="tag-filter-list">
                    {filteredTagOptions.length === 0 ? (
                      <p className="tag-filter-empty">没有匹配的标签</p>
                    ) : (
                      filteredTagOptions.map((tag) => {
                        const active = activeTagIds.includes(tag.id);
                        return (
                          <button
                            key={tag.id}
                            type="button"
                            className={`tag-filter-option${
                              active ? " is-active" : ""
                            }`}
                            onClick={() => toggleTagFilter(tag.id)}
                            disabled={tag.count === 0 && !active}
                            title={
                              tag.count === 0
                                ? "这个标签还没有被使用"
                                : `${tag.count} 个画集`
                            }
                          >
                            <span className="collection-check">
                              {active && <CheckIcon width={12} height={12} />}
                            </span>
                            <span className="tag-filter-name">
                              {tag.translatedName || tag.name}
                              {tag.translatedName &&
                                tag.translatedName !== tag.name && (
                                  <small>{tag.name}</small>
                                )}
                            </span>
                            <span className="tag-filter-count">
                              {tag.count}
                            </span>
                          </button>
                        );
                      })
                    )}
                  </div>
                  <div className="tag-filter-mode">
                    <span>匹配方式</span>
                    <div className="segmented segmented-mini">
                      <button
                        type="button"
                        className={tagMode === "and" ? "is-active" : ""}
                        onClick={() => setTagMode("and")}
                        title="AND：画集必须同时包含所有选中标签"
                      >
                        AND 同时含
                      </button>
                      <button
                        type="button"
                        className={tagMode === "or" ? "is-active" : ""}
                        onClick={() => setTagMode("or")}
                        title="OR：画集包含任意一个选中标签即可"
                      >
                        OR 任一
                      </button>
                      <button
                        type="button"
                        className={tagMode === "exclude" ? "is-active" : ""}
                        onClick={() => setTagMode("exclude")}
                        title="排除：隐藏包含选中标签的画集"
                      >
                        排除
                      </button>
                    </div>
                  </div>
                  <div className="tag-filter-guide">
                    <p>
                      <strong>空</strong>
                      <span>一个标签都不选，不按标签筛选</span>
                    </p>
                    <p>
                      <strong>AND</strong>
                      <span>同时含：每一枚已选标签都必须存在</span>
                    </p>
                    <p>
                      <strong>OR</strong>
                      <span>任一：包含任意一枚已选标签即可</span>
                    </p>
                    <p>
                      <strong>排除</strong>
                      <span>隐藏包含任意一枚已选标签的画集</span>
                    </p>
                  </div>
                  <div className="tag-filter-foot">
                    <span>
                      {tags.length} 个标签 ·{" "}
                      {tagMode === "and"
                        ? "多选为同时包含"
                        : tagMode === "or"
                          ? "多选为任一匹配"
                          : "多选为排除这些标签"}
                    </span>
                    <button
                      type="button"
                      className="btn btn-quiet btn-compact"
                      onClick={() => setActiveTagIds([])}
                      disabled={!hasTagFilter}
                    >
                      清除
                    </button>
                  </div>
                </div>
              )}
            </div>

            <div className="tag-filter-wrap" ref={filterRef}>
              <button
                type="button"
                className={`btn btn-ghost btn-compact${
                  hasAdvancedFilter ? " is-filtering" : ""
                }`}
                onClick={() => {
                  setTagFilterOpen(false);
                  setFilterOpen((current) => !current);
                }}
                title="组合筛选：评分范围、阅读状态、缺失文件，可与标签、收藏夹和搜索叠加"
              >
                <FilterIcon width={15} height={15} />
                筛选
                {filterCount > 0 && (
                  <span className="filter-count">{filterCount}</span>
                )}
              </button>

              {filterOpen && (
                <div className="tag-filter-popover filter-popover">
                  <div className="filter-section">
                    <div className="filter-section-head">
                      <span>评分范围</span>
                      <small>可多选</small>
                    </div>
                    <div className="filter-chip-grid">
                      {RATING_FILTER_OPTIONS.map((option) => {
                        const active = ratingBuckets.includes(option.bucket);
                        return (
                          <button
                            key={option.bucket}
                            type="button"
                            className={`filter-chip${active ? " is-active" : ""}`}
                            onClick={() => toggleRatingBucket(option.bucket)}
                            title={`评分 ${RATING_GROUP_LABELS[option.bucket]}`}
                          >
                            {active && <CheckIcon width={11} height={11} />}
                            {option.label}
                          </button>
                        );
                      })}
                    </div>
                  </div>

                  <div className="filter-section">
                    <div className="filter-section-head">
                      <span>阅读状态</span>
                      <small>可多选</small>
                    </div>
                    <div className="filter-chip-grid filter-chip-grid-read">
                      {READ_STATUS_OPTIONS.map((option) => {
                        const active = readStatuses.includes(option.value);
                        return (
                          <button
                            key={option.value}
                            type="button"
                            className={`filter-chip${active ? " is-active" : ""}`}
                            onClick={() => toggleReadStatus(option.value)}
                          >
                            {active && <CheckIcon width={11} height={11} />}
                            {option.label}
                          </button>
                        );
                      })}
                    </div>
                  </div>

                  <label className="filter-check">
                    <input
                      type="checkbox"
                      checked={missingOnly}
                      onChange={(event) =>
                        setMissingOnly(event.target.checked)
                      }
                    />
                    <span>只看文件缺失的画集</span>
                  </label>

                  <p className="filter-hint">
                    不同条件之间取交集；同一条件内多选为任一匹配，可与标签、收藏夹和搜索叠加。
                  </p>

                  <div className="tag-filter-foot">
                    <span>
                      {hasAdvancedFilter
                        ? `已选 ${filterCount} 项`
                        : "尚未选择筛选条件"}
                    </span>
                    <button
                      type="button"
                      className="btn btn-quiet btn-compact"
                      onClick={clearAdvancedFilters}
                      disabled={!hasAdvancedFilter}
                    >
                      重置
                    </button>
                  </div>
                </div>
              )}
            </div>

            <label className="select-field">
              <span>分组</span>
              <select
                value={groupBy}
                onChange={(event) =>
                  setGroupBy(event.target.value as GroupMode)
                }
                title="按标签分组时，每个画集归入自然排序最靠前的标签；按收藏夹分组时优先默认收藏夹、再按收藏夹顺序；未分类/未收藏排在最后"
              >
                <option value="none">不分组</option>
                <option value="rating">评分</option>
                <option value="tag">标签</option>
                <option value="collection">收藏夹</option>
              </select>
            </label>

            <label className="select-field">
              <span>排序</span>
              <select
                value={sort}
                onChange={(event) => setSort(event.target.value as SortKey)}
              >
                {SORT_OPTIONS.map((option) => (
                  <option key={option.value} value={option.value}>
                    {option.label}
                  </option>
                ))}
              </select>
            </label>

            <button
              className="btn btn-ghost btn-compact"
              onClick={() =>
                setDirection((current) => (current === "asc" ? "desc" : "asc"))
              }
              title={direction === "asc" ? "当前升序" : "当前降序"}
            >
              <SwapIcon width={15} height={15} />
              {direction === "asc" ? "升序" : "降序"}
            </button>

            <div className="segmented" aria-label="浏览模式">
              <button
                className={viewMode === "scroll" ? "is-active" : ""}
                onClick={() => setViewMode("scroll")}
                type="button"
              >
                <GridIcon width={15} height={15} />
                滚动
              </button>
              <button
                className={viewMode === "pages" ? "is-active" : ""}
                onClick={() => setViewMode("pages")}
                type="button"
              >
                <BookIcon width={15} height={15} />
                分页
              </button>
            </div>

            <label className="select-field">
              <span>每页</span>
              <select
                value={pageSize}
                onChange={(event) => setPageSize(Number(event.target.value))}
              >
                {PAGE_SIZES.map((size) => (
                  <option key={size} value={size}>
                    {size}
                  </option>
                ))}
              </select>
            </label>

            {viewMode === "pages" && (
              <>
                <button
                  type="button"
                  className={`btn btn-ghost btn-compact auto-toggle${
                    autoPaging ? " is-active" : ""
                  }`}
                  disabled={totalPages <= 1}
                  aria-pressed={autoPaging}
                  title={
                    autoPaging
                      ? "暂停自动翻页"
                      : `每 ${autoPageSeconds} 秒自动翻到下一页`
                  }
                  onClick={() => setAutoPaging((value) => !value)}
                >
                  {autoPaging ? (
                    <PauseIcon width={14} height={14} />
                  ) : (
                    <PlayIcon width={14} height={14} />
                  )}
                  {autoPaging ? "暂停" : "自动翻页"}
                </button>
                <label className="select-field">
                  <span>间隔</span>
                  <select
                    value={autoPageSeconds}
                    disabled={totalPages <= 1}
                    onChange={(event) =>
                      setAutoPageSeconds(Number(event.target.value))
                    }
                  >
                    {AUTO_PAGE_SECONDS.map((seconds) => (
                      <option key={seconds} value={seconds}>
                        {seconds} 秒
                      </option>
                    ))}
                  </select>
                </label>
              </>
            )}
          </div>
        </header>

        <div
          className="content-shell"
          style={{ ["--drawer-w" as string]: `${drawerWidth}px` } as React.CSSProperties}
        >
          <div className="content-scroll" ref={contentRef}>
          {hasFilter && (
            <div className="active-filter-bar">
              {hasTagFilter && (
                <Fragment>
                  <span className="active-filter-label">
                    {tagMode === "and"
                      ? "标签筛选 · 同时含"
                      : tagMode === "or"
                        ? "标签筛选 · 任一"
                        : "标签筛选 · 排除"}
                  </span>
                  {activeTags.map((tag) => (
                    <button
                      key={tag.id}
                      type="button"
                      className="active-filter-chip"
                      onClick={() => toggleTagFilter(tag.id)}
                      title={`移除标签 ${tag.name}`}
                    >
                      {tag.translatedName || tag.name}
                      <CloseIcon width={11} height={11} />
                    </button>
                  ))}
                </Fragment>
              )}
              {hasRatingFilter && (
                <Fragment>
                  <span className="active-filter-label">评分</span>
                  {activeRatingBuckets.map((option) => (
                    <button
                      key={option.bucket}
                      type="button"
                      className="active-filter-chip"
                      onClick={() => toggleRatingBucket(option.bucket)}
                      title={`移除评分范围 ${option.label}`}
                    >
                      {option.label}
                      <CloseIcon width={11} height={11} />
                    </button>
                  ))}
                </Fragment>
              )}
              {hasReadFilter && (
                <Fragment>
                  <span className="active-filter-label">阅读</span>
                  {activeReadStatuses.map((option) => (
                    <button
                      key={option.value}
                      type="button"
                      className="active-filter-chip"
                      onClick={() => toggleReadStatus(option.value)}
                      title={`移除阅读状态 ${option.label}`}
                    >
                      {option.label}
                      <CloseIcon width={11} height={11} />
                    </button>
                  ))}
                </Fragment>
              )}
              {missingOnly && (
                <button
                  type="button"
                  className="active-filter-chip"
                  onClick={() => setMissingOnly(false)}
                  title="移除缺失文件筛选"
                >
                  仅缺失文件
                  <CloseIcon width={11} height={11} />
                </button>
              )}
              <button
                type="button"
                className="btn btn-quiet btn-compact"
                onClick={clearAllFilters}
              >
                清除全部
              </button>
            </div>
          )}

          {listError && (
            <div className="inline-alert" role="alert">
              <AlertIcon />
              <span>{listError}</span>
              <button
                className="btn btn-ghost btn-compact"
                onClick={() => void loadPage(1, true, true)}
              >
                重试
              </button>
            </div>
          )}

          {loading && items.length === 0 ? (
            <SkeletonGrid />
          ) : total === 0 && searchTerm ? (
            <div className="empty-state">
              <div className="empty-icon">
                <SearchIcon width={28} height={28} />
              </div>
              <h2>没有找到“{searchTerm}”</h2>
              <p>
                搜索会匹配标题、文件名和备注。三个字以上走全文索引，一两个字自动退化为模糊匹配。
              </p>
              <div className="empty-actions">
                <button
                  className="btn btn-primary btn-large"
                  onClick={() => setSearch("")}
                >
                  <CloseIcon />
                  清除搜索
                </button>
              </div>
            </div>
          ) : total === 0 && hasAdvancedFilter ? (
            <div className="empty-state">
              <div className="empty-icon">
                <FilterIcon width={30} height={30} />
              </div>
              <h2>没有符合筛选条件的画集</h2>
              <p>
                当前条件：
                {[
                  hasRatingFilter
                    ? `评分 ${activeRatingBuckets.map((option) => option.label).join(" / ")}`
                    : null,
                  hasReadFilter
                    ? `阅读 ${activeReadStatuses.map((option) => option.label).join(" / ")}`
                    : null,
                  missingOnly ? "仅缺失文件" : null,
                  hasTagFilter
                    ? `标签 ${activeTags.map((tag) => `#${tag.translatedName || tag.name}`).join(" ")}`
                    : null,
                ]
                  .filter(Boolean)
                  .join(" · ")}
                。可以减少条件，或清除筛选后重新浏览。
              </p>
              <div className="empty-actions">
                <button
                  className="btn btn-primary btn-large"
                  onClick={clearAllFilters}
                >
                  <CloseIcon />
                  清除筛选
                </button>
              </div>
            </div>
          ) : total === 0 && hasTagFilter ? (
            <div className="empty-state">
              <div className="empty-icon">
                <TagIcon width={30} height={30} />
              </div>
              <h2>没有匹配的画集</h2>
              <p>
                {tagMode === "exclude"
                  ? "排除这些标签后没有剩余画集："
                  : tagMode === "or"
                    ? "没有任何画集包含这些标签中的任意一个："
                    : "当前筛选要求画集同时包含："}
                {activeTags
                  .map((tag) => `#${tag.translatedName || tag.name}`)
                  .join("、")}
                。可以移除部分标签，或清除筛选后再看。
              </p>
              <div className="empty-actions">
                <button
                  className="btn btn-primary btn-large"
                  onClick={() => setActiveTagIds([])}
                >
                  <BookIcon />
                  清除标签筛选
                </button>
                <button
                  className="btn btn-ghost btn-large"
                  onClick={() => setTagCenterOpen(true)}
                >
                  <TagIcon />
                  打开标签中心
                </button>
              </div>
            </div>
          ) : total === 0 && activeCollection ? (
            <div className="empty-state">
              <div className="empty-icon">
                <HeartIcon width={30} height={30} />
              </div>
              <h2>收藏夹还是空的</h2>
              <p>
                打开任意画集详情，在「收藏夹」里勾选「{activeCollection.name}
                」，画集就会出现在这里。
              </p>
              <div className="empty-actions">
                <button
                  className="btn btn-primary btn-large"
                  onClick={() => setActiveCollectionId(null)}
                >
                  <BookIcon />
                  浏览全部画集
                </button>
              </div>
            </div>
          ) : total === 0 ? (
            <div className="empty-state">
              <div className="empty-icon">
                <LogoIcon width={34} height={34} />
              </div>
              <h2>画廊还是空的</h2>
              <p>
                把 ZIP / CBZ 压缩包导入进来。画集会复制到库目录，源文件保持不动。
              </p>
              <div className="empty-actions">
                <button
                  className="btn btn-primary btn-large"
                  onClick={() => void pickFiles()}
                  disabled={importing}
                >
                  <ImportIcon />
                  导入压缩包
                </button>
                <button
                  className="btn btn-ghost btn-large"
                  onClick={() => void pickFolder()}
                  disabled={importing}
                >
                  <FolderIcon />
                  导入文件夹
                </button>
              </div>
            </div>
          ) : (
            <>
              {seriesList.length > 0 && page === 1 && !selectionMode && (
                <section className="series-strip" aria-label="合集">
                  <header className="series-strip-head">
                    <span className="eyebrow">SERIES</span>
                    <strong>合集 · {seriesList.length}</strong>
                  </header>
                  <div className="series-strip-grid">
                    {seriesList.map((entry) => (
                      <SeriesCard
                        key={entry.id}
                        series={entry}
                        onOpen={(next) => {
                          // 合集详情与画廊详情是两块独立浮层，打开一个就收另一个。
                          setSelected(null);
                          setOpenSeries(next);
                        }}
                      />
                    ))}
                  </div>
                </section>
              )}
              <div className="archive-grid">
                {groupedItems.map((group, groupIndex) => (
                  <Fragment
                    key={
                      group.bucket === -1
                        ? "all"
                        : `${groupBy}-${group.bucket}-${groupIndex}`
                    }
                  >
                    {groupBy === "rating" && (
                      <div className="archive-group-header">
                        <span className="archive-group-title">
                          <StarIcon width={14} height={14} />
                          <strong>
                            {RATING_GROUP_LABELS[group.bucket] ??
                              `${group.bucket}`}
                          </strong>
                        </span>
                        <span className="archive-group-count">
                          {groupCounts?.[group.bucket] ?? group.items.length}{" "}
                          个画集
                        </span>
                      </div>
                    )}
                    {groupBy === "tag" && (
                      <div
                        className={`archive-group-header${
                          group.bucket === 0 ? " is-uncategorized" : ""
                        }`}
                      >
                        <span className="archive-group-title">
                          <TagIcon width={14} height={14} />
                          <strong>
                            {group.items[0]?.groupLabel ?? "未分类"}
                          </strong>
                          {group.items[0]?.groupCategory && (
                            <span className="archive-group-category">
                              {group.items[0].groupCategory}
                            </span>
                          )}
                        </span>
                        <span className="archive-group-count">
                          {groupCounts?.[group.bucket] ?? group.items.length}{" "}
                          个画集
                        </span>
                      </div>
                    )}
                    {groupBy === "collection" && (
                      <div
                        className={`archive-group-header${
                          group.bucket === 0 ? " is-uncategorized" : ""
                        }`}
                      >
                        <span className="archive-group-title">
                          <HeartIcon width={14} height={14} />
                          <strong>
                            {group.items[0]?.groupLabel ?? "未收藏"}
                          </strong>
                        </span>
                        <span className="archive-group-count">
                          {groupCounts?.[group.bucket] ?? group.items.length}{" "}
                          个画集
                        </span>
                      </div>
                    )}
                    {group.items.map((card) => (
                      <ArchiveCard
                        key={card.id}
                        card={card}
                        onOpen={setSelected}
                        selectionMode={selectionMode}
                        selected={selectedIds.includes(card.id)}
                        onToggleSelect={toggleSelect}
                        onQuickEdit={setQuickEditCard}
                        onRead={setReaderCard}
                        eager={viewMode === "pages"}
                        onQueueMove={
                          activeQueue && !selectionMode
                            ? (direction) => void moveQueueCard(card.id, direction)
                            : undefined
                        }
                        onContextMenu={openCardMenu}
                        queueEdges={
                          queueEdgeIds
                            ? {
                                upDisabled: card.id === queueEdgeIds.first,
                                downDisabled: card.id === queueEdgeIds.last,
                              }
                            : undefined
                        }
                      />
                    ))}
                  </Fragment>
                ))}
              </div>

              {viewMode === "scroll" ? (
                <div className="load-area" ref={sentinelRef}>
                  {loadingMore ? (
                    <span className="load-status">
                      <span className="spinner" />
                      正在加载更多…
                    </span>
                  ) : hasMore ? (
                    <button
                      className="btn btn-ghost"
                      onClick={() => void loadPage(page + 1, false)}
                    >
                      加载更多
                    </button>
                  ) : (
                    <span className="load-status">
                      已加载全部 {total} 个画集
                    </span>
                  )}
                </div>
              ) : (
                <>
                  <div className="pager">
                  <button
                    className="btn btn-ghost"
                    disabled={page <= 1 || loading}
                    onClick={() => goToPage(page - 1)}
                  >
                    <ChevronLeftIcon />
                    上一页
                  </button>
                  <span className="pager-status" aria-live="polite">
                    {loading && <span className="spinner" />}
                    第 {page} / {totalPages} 页
                  </span>
                  <button
                    className="btn btn-ghost"
                    disabled={page >= totalPages || loading}
                    onClick={() => goToPage(page + 1)}
                  >
                    下一页
                    <ChevronRightIcon />
                  </button>
                  </div>
                  <p className="pager-hint">
                    ← ↑ 上一页 · → ↓ 下一页 · 点击两侧边缘 · 滚轮上下左右翻页
                  </p>
                </>
              )}

              {selectionMode && (
                <div className="selection-bar">
                  <span className="selection-count">
                    已选 {selectedIds.length} 个画集
                  </span>
                  <select
                    value={batchCollectionId}
                    aria-label="选择收藏夹"
                    onChange={(event) =>
                      setBatchCollectionId(
                        event.target.value
                          ? Number(event.target.value)
                          : "",
                      )
                    }
                  >
                    <option value="">选择收藏夹…</option>
                    {collections.map((collection) => (
                      <option key={collection.id} value={collection.id}>
                        {collection.name}（{collection.count}）
                      </option>
                    ))}
                  </select>
                  <button
                    type="button"
                    className="btn btn-ghost btn-compact"
                    onClick={() => setCollectionCreateIntent("batch")}
                    title="新建收藏夹并命名"
                    disabled={batchBusy}
                  >
                    <PlusIcon width={13} height={13} />
                    新建
                  </button>
                  <button
                    type="button"
                    className="btn btn-primary btn-compact"
                    disabled={
                      selectedIds.length === 0 ||
                      !batchCollectionId ||
                      batchBusy
                    }
                    onClick={() => void addSelectedToCollection()}
                  >
                    <HeartIcon width={14} height={14} />
                    {batchBusy ? "加入中…" : "加入收藏夹"}
                  </button>
                  <select
                    value={batchQueueId}
                    aria-label="选择待看目录"
                    onChange={(event) =>
                      setBatchQueueId(
                        event.target.value ? Number(event.target.value) : "",
                      )
                    }
                  >
                    <option value="">选择待看目录…</option>
                    {queues.map((queue) => (
                      <option key={queue.id} value={queue.id}>
                        {queue.name}（{queue.count}）
                      </option>
                    ))}
                  </select>
                  <button
                    type="button"
                    className="btn btn-primary btn-compact"
                    disabled={
                      selectedIds.length === 0 || !batchQueueId || batchBusy
                    }
                    onClick={() => void addSelectedToQueue()}
                    title="按选择顺序追加进待看目录，数量不限"
                  >
                    <PlusIcon width={14} height={14} />
                    {batchBusy ? "加入中…" : "加入待看"}
                  </button>
                  <button
                    type="button"
                    className="btn btn-ghost btn-compact"
                    disabled={selectedIds.length < 2 || batchBusy}
                    onClick={() => setSeriesCreateOpen(true)}
                    title="把选中的画集合成一个合集：主网格只留一张轮换封面的卡，详情里可调顺序与集数"
                  >
                    <BookIcon width={14} height={14} />
                    合成合集
                  </button>
                  <button
                    type="button"
                    className={`btn btn-compact${
                      batchDeleteConfirm ? " btn-danger" : " btn-quiet"
                    }`}
                    disabled={selectedIds.length === 0 || batchBusy}
                    onClick={() => void deleteSelected()}
                    title={
                      batchDeleteConfirm
                        ? "再次点击确认移入回收站"
                        : "把选中的画集移入回收站，不会删除原始文件"
                    }
                  >
                    <TrashIcon width={14} height={14} />
                    {batchDeleteConfirm ? "确认删除" : "移入回收站"}
                  </button>
                  <button
                    type="button"
                    className="btn btn-ghost btn-compact"
                    onClick={exitSelection}
                  >
                    完成
                  </button>
                </div>
              )}
            </>
          )}
          </div>

          {viewMode === "pages" && warmCovers.length > 0 && (
            <div className="cover-warm" aria-hidden="true">
              {warmCovers.map((url) => (
                <img
                  key={url}
                  src={url}
                  alt=""
                  decoding="sync"
                  loading="eager"
                  draggable={false}
                />
              ))}
            </div>
          )}

          {viewMode === "pages" && totalPages > 1 && (
            <>
              <button
                type="button"
                className="page-rail is-prev"
                title="上一页（← / ↑）"
                aria-label="上一页"
                disabled={page <= 1 || loading}
                onClick={() => goToPage(page - 1)}
              >
                <ChevronLeftIcon width={22} height={22} />
              </button>
              <button
                type="button"
                className="page-rail is-next"
                title="下一页（→ / ↓）"
                aria-label="下一页"
                disabled={page >= totalPages || loading}
                onClick={() => goToPage(page + 1)}
              >
                <ChevronRightIcon width={22} height={22} />
              </button>
            </>
          )}

          {/* 详情「外拓」停靠：作为 content-shell 的 flex 兄弟节点渲染，
              画廊网格 repeat(auto-fill) 自动让位重排，不铺遮罩不遮挡。 */}
          {selected && detailDock === "dock" && (
            <>
              <Splitter
                label="详情面板"
                value={drawerWidth}
                min={320}
                max={760}
                direction={-1}
                onChange={setDrawerWidth}
                onReset={resetDrawerWidth}
              />
              <DetailDrawer
                docked
                dock={detailDock}
                onDockChange={setDetailDock}
                card={selected}
                collections={collections}
                onClose={() => setSelected(null)}
                onRead={() => {
                  setReaderCard(selected);
                  setSelected(null);
                }}
                onNotify={notify}
                onRatingChange={updateRating}
                onCollectionToggle={toggleCollectionMembership}
                onCollectionCover={setCollectionCover}
                onNoteChange={updateNote}
                onTagsChanged={loadTags}
                onDelete={removeArchive}
              />
            </>
          )}
        </div>
      </main>

      {selected && detailDock === "float" && (
        <DetailDrawer
          dock={detailDock}
          onDockChange={setDetailDock}
          card={selected}
          collections={collections}
          onClose={() => setSelected(null)}
          onRead={() => {
            setReaderCard(selected);
            setSelected(null);
          }}
          onNotify={notify}
          onRatingChange={updateRating}
          onCollectionToggle={toggleCollectionMembership}
          onCollectionCover={setCollectionCover}
          onNoteChange={updateNote}
          onTagsChanged={loadTags}
          onDelete={removeArchive}
        />
      )}

      {seriesCreateOpen && (
        <CollectionNameDialog
          title="新建合集"
          noun="合集"
          placeholder="合集名，例如「入り浸りギャル」"
          presets={[]}
          busy={seriesBusy}
          existingNames={seriesList.map((entry) => entry.name)}
          onCreate={(name) => void createSeriesFromSelection(name)}
          onClose={() => setSeriesCreateOpen(false)}
        />
      )}

      {openSeries && (
        <div
          className="series-drawer-backdrop"
          onClick={() => setOpenSeries(null)}
          aria-hidden="true"
        />
      )}

      {openSeries && (
        <SeriesDrawer
          series={openSeries}
          onClose={() => setOpenSeries(null)}
          onChanged={() => {
            void (async () => {
              try {
                const list = await api.listSeries();
                setSeriesList(list);
                setOpenSeries(
                  list.find((entry) => entry.id === openSeries.id) ?? null,
                );
              } catch {
                // 拉不到就保持原样，下次操作会再试
              }
              void loadPage(page, true, true);
            })();
          }}
          onReadArchive={(archiveId) => {
            void (async () => {
              try {
                const result = await api.queryArchives({
                  archiveIds: [archiveId],
                  pageSize: 12,
                });
                const card = result.items[0];
                if (!card) {
                  notify("这一话已经不在库里了", "error");
                  return;
                }
                // 记下这合集的集数顺序，阅读器读完一话自动接下一话。
                setReaderSeriesIds(
                  openSeries.items.map((entry) => entry.archiveId),
                );
                setReaderCard(card);
                setOpenSeries(null);
              } catch (error) {
                notify(`打开失败：${String(error)}`, "error");
              }
            })();
          }}
          onNotify={notify}
        />
      )}

      {bgMenu && (
        <ReaderMenu
          x={bgMenu.x}
          y={bgMenu.y}
          entries={[
            { label: "导入压缩包", onClick: () => void pickFiles() },
            { label: "导入文件夹", onClick: () => void pickFolder() },
            {
              label: "刷新当前视图",
              onClick: () => void loadPage(page, true, true),
            },
            {
              label:
                viewMode === "pages" ? "切换为网格滚动" : "切换为分页浏览",
              dividerBefore: true,
              onClick: () => setViewMode(viewMode === "pages" ? "scroll" : "pages"),
            },
            {
              label:
                detailDock === "dock" ? "详情浮层改为浮动" : "详情浮层改为停靠",
              onClick: () => setDetailDock(detailDock === "dock" ? "float" : "dock"),
            },
            { label: "打开标签中心", dividerBefore: true, onClick: () => setTagCenterOpen(true) },
            { label: "打开网页抓取", onClick: () => setD2PanelOpen(true) },
            { label: "打开更新面板", onClick: () => setReleasePanelOpen(true) },
            {
              label: "在资源管理器中显示库目录",
              onClick: () => {
                if (library.libraryPath) void api.revealInExplorer(library.libraryPath);
              },
            },
            { label: "设置…", dividerBefore: true, onClick: () => setSettingsOpen(true) },
          ]}
          onClose={() => setBgMenu(null)}
        />
      )}

      {styleEditing && (
        <>
          <div
            className="style-popover-backdrop"
            onClick={() => setStyleEditing(null)}
          />
          {(() => {
            const editingCollection = collections.find(
              (entry) => entry.id === styleEditing.id,
            );
            if (!editingCollection) return null;
            const applyStyle = (icon: CollectionIcon, color: string) => {
              void api
                .updateCollectionStyle(editingCollection.id, icon, color)
                .then(() => {
                  setCollections((previous) =>
                    previous.map((entry) =>
                      entry.id === editingCollection.id
                        ? { ...entry, icon, color }
                        : entry,
                    ),
                  );
                })
                .catch((error) =>
                  notify(`更新图标失败：${String(error)}`, "error"),
                );
            };
            return (
              <div
                className="style-popover"
                style={{ left: styleEditing.x, top: styleEditing.y }}
              >
                <div className="style-popover-row">
                  {COLLECTION_ICONS.map((icon) => (
                    <button
                      key={icon}
                      type="button"
                      className={`style-shape${
                        editingCollection.icon === icon ? " is-active" : ""
                      }`}
                      title={
                        icon === "star"
                          ? "五角星"
                          : icon === "heart"
                            ? "爱心"
                            : icon === "circle"
                              ? "圆形"
                              : "正方形"
                      }
                      onClick={() =>
                        applyStyle(icon, editingCollection.color)
                      }
                    >
                      <CollectionGlyph
                        icon={icon}
                        color={editingCollection.color}
                        size={18}
                      />
                    </button>
                  ))}
                </div>
                <div className="style-popover-row">
                  {COLLECTION_COLORS.map((color) => (
                    <button
                      key={color}
                      type="button"
                      className={`style-swatch${
                        editingCollection.color === color ? " is-active" : ""
                      }`}
                      style={{ background: color }}
                      title={color}
                      aria-label={`填充色 ${color}`}
                      onClick={() =>
                        applyStyle(editingCollection.icon, color)
                      }
                    />
                  ))}
                </div>
              </div>
            );
          })()}
        </>
      )}

      {settingsOpen && (
        <SettingsDialog
          library={library}
          viewMode={viewMode}
          onViewModeChange={setViewMode}
          detailDock={detailDock}
          onDetailDockChange={setDetailDock}
          onOpenTagCenter={() => {
            setSettingsOpen(false);
            setTagCenterOpen(true);
          }}
          onOpenD2={() => {
            setSettingsOpen(false);
            setD2PanelOpen(true);
          }}
          onOpenRelease={() => {
            setSettingsOpen(false);
            setReleasePanelOpen(true);
          }}
          onClose={() => setSettingsOpen(false)}
        />
      )}

      {readerCard && (
        <Reader
          card={readerCard}
          onClose={() => {
            setReaderCard(null);
            setReaderSeriesIds(null);
          }}
          onNotify={notify}
          onProgress={updateReaderProgress}
          onSwitchArchive={switchReaderArchive}
          hasPrevArchive={
            readerSeriesNav
              ? readerSeriesNav.prev != null
              : readerIndex > 0 || (readerIndex === 0 && page > 1)
          }
          hasNextArchive={
            readerSeriesNav
              ? readerSeriesNav.next != null
              : readerIndex >= 0 &&
                (readerIndex < items.length - 1 ||
                  page < totalPagesRef.current)
          }
          continuousNext={readerSeriesNav != null}
          onOpenDetail={() => setReaderDetailOpen(true)}
          detailOpen={readerDetailOpen}
        />
      )}

      {readerCard && readerDetailOpen && (
        <DetailDrawer
          card={readerCard}
          collections={collections}
          embeddedInReader
          dock="float"
          onClose={() => setReaderDetailOpen(false)}
          onRead={() => setReaderDetailOpen(false)}
          onNotify={notify}
          onRatingChange={updateRating}
          onCollectionToggle={toggleCollectionMembership}
          onCollectionCover={setCollectionCover}
          onNoteChange={updateNote}
          onTagsChanged={loadTags}
          onDelete={async (id) => {
            await removeArchive(id);
            setReaderDetailOpen(false);
            setReaderCard(null);
          }}
        />
      )}

      {quickEditCard && (
        <CollectionEditor
          card={quickEditCard}
          collections={collections}
          onClose={() => setQuickEditCard(null)}
          onOpenDetail={() => {
            setSelected(quickEditCard);
            setQuickEditCard(null);
          }}
          onChanged={refreshCollectionsAfterEdit}
          onNotify={notify}
        />
      )}

      {dragActive && !importOpen && (
        <div className="drop-overlay" aria-hidden="true">
          <div className="drop-card">
            <span className="drop-icon">
              <ImportIcon width={30} height={30} />
            </span>
            <strong>松开后核对并导入</strong>
            <p>支持 ZIP / CBZ 压缩包，或包含压缩包的文件夹</p>
            <small>会先列出清单与标签，确认之后才写入库</small>
          </div>
        </div>
      )}

      {tagCenterOpen && (
        <TagCenter
          onClose={() => setTagCenterOpen(false)}
          onNotify={notify}
          onChanged={loadTags}
          onBrowseTag={browseTag}
        />
      )}

      {trashOpen && (
        <TrashCenter
          onClose={() => setTrashOpen(false)}
          onChanged={refreshAfterTrashChange}
          onNotify={notify}
        />
      )}

        {cardMenu && (
          <div
            className="ctx-backdrop"
            onClick={closeCardMenu}
            onContextMenu={(event) => {
              event.preventDefault();
              closeCardMenu();
            }}
          >
            <div
              className="ctx-menu"
              style={{ left: cardMenu.x, top: cardMenu.y }}
              role="menu"
              aria-label="画集操作菜单"
            >
              <div className="ctx-title">{cardMenu.card.title}</div>
              <button
                type="button"
                onClick={() => {
                  setReaderCard(cardMenu.card);
                  closeCardMenu();
                }}
              >
                开始阅读
              </button>
              <button
                type="button"
                onClick={() => {
                  setSelected(cardMenu.card);
                  closeCardMenu();
                }}
              >
                查看详情
              </button>
              <button
                type="button"
                onClick={() => {
                  void addCardToQueue(cardMenu.card);
                  closeCardMenu();
                }}
              >
                加入待看目录
              </button>
              <button
                type="button"
                onClick={() => {
                  setQuickEditCard(cardMenu.card);
                  closeCardMenu();
                }}
              >
                收藏到…
              </button>
              <button
                type="button"
                onClick={() => {
                  setSelectionMode(true);
                  setSelectedIds([cardMenu.card.id]);
                  closeCardMenu();
                }}
              >
                进入多选
              </button>
              <button
                type="button"
                className="danger"
                onClick={() => {
                  void trashFromMenu(cardMenu.card);
                  closeCardMenu();
                }}
              >
                移入回收站
              </button>
            </div>
          </div>
        )}
      {queuePanelOpen && (
        <QueuePanel
          tags={tags}
          onClose={() => setQueuePanelOpen(false)}
          onNotify={notify}
          onOpenQueue={openQueue}
          onChanged={() => void loadPage(1, true, true)}
        />
      )}

      {releasePanelOpen && (
        <ReleasePanel
          onClose={() => setReleasePanelOpen(false)}
          onNotify={notify}
        />
      )}

      {d2PanelOpen && (
        <D2Panel
          onClose={() => setD2PanelOpen(false)}
          onNotify={notify}
        />
      )}

      {collectionCreateIntent !== null && (
        <CollectionNameDialog
          busy={savingCollection}
          existingNames={collections.map((collection) => collection.name)}
          onCreate={(name) => void submitCollection(name)}
          onClose={() => {
            if (!savingCollection) setCollectionCreateIntent(null);
          }}
        />
      )}

      {importOpen && (
        <ImportDialog
          ref={importDialogRef}
          initialPaths={importSeedPaths}
          collections={collections}
          knownTags={tags}
          dragActive={dragActive}
          importing={importing}
          progress={importProgress}
          onSubmit={(payload) => void startImport(payload)}
          onClose={() => {
            setImportOpen(false);
            setImportSeedPaths([]);
          }}
          onPickFiles={() => void pickFiles()}
          onPickFolder={() => void pickFolder()}
          onNotify={notify}
        />
      )}

      {importReport && (
        <ImportResultDialog
          report={importReport}
          onClose={() => setImportReport(null)}
        />
      )}

      {toast && <Toast toast={toast} onClose={() => setToast(null)} />}
    </div>
  );
}

function SkeletonGrid() {
  return (
    <div className="archive-grid" aria-label="正在加载画集">
      {Array.from({ length: 12 }).map((_, index) => (
        <div className="archive-card skeleton-card" key={index}>
          <div className="skeleton-cover" />
          <div className="archive-body">
            <div className="skeleton-line skeleton-line-title" />
            <div className="skeleton-line" />
          </div>
        </div>
      ))}
    </div>
  );
}
