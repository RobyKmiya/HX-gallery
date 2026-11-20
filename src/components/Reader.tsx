import {
  memo,
  useCallback,
  useEffect,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
  type SyntheticEvent as ReactSyntheticEvent,
} from "react";
import { api, animFrameUrl, readerPageUrl } from "../api";
import type {
  AnimMeta,
  ArchiveCardData,
  ReaderBookmark,
  ReaderInfo,
  ReaderPage,
} from "../types";
import {
  AlertIcon,
  BookIcon,
  ChevronLeftIcon,
  ChevronRightIcon,
  CloseIcon,
  GridIcon,
  PauseIcon,
  PlayIcon,
  StarIcon,
  TagIcon,
  TrashIcon,
} from "./Icons";

interface ReaderProps {
  card: ArchiveCardData;
  onClose: () => void;
  onNotify: (message: string, kind?: "info" | "success" | "error") => void;
  onProgress?: (archiveId: number, lastPage: number) => void;
  /**
   * 直接切到列表里的上一本 / 下一本，省掉「关阅读器 → 回列表 → 再点开」。
   * 传 null 表示已经是头/尾。由 LibraryView 按当前列表顺序算出来。
   */
  onSwitchArchive?: (direction: -1 | 1) => void;
  hasPrevArchive?: boolean;
  hasNextArchive?: boolean;
  /** 在阅读器里打开详情浮层（LibraryView 挂 DetailDrawer）。「详情 (I)」按钮。 */
  onOpenDetail?: () => void;
  /** 详情浮层开着时置真：Esc 交给浮层处理，阅读器不抢。 */
  detailOpen?: boolean;
}

type ZoomMode = "fit" | "width" | "actual";

const CACHE_LIMIT = 14;
const PREFETCH_AHEAD = 4;
const PREFETCH_BEHIND = 1;
/**
 * 超过这个体积的页不预取。
 *
 * 动图（animated webp/gif）单页能到二三十兆：预取 4 页就是上百兆，既超过
 * 后端 96MB 的字节缓存预算（会把正在看的那页挤出去），也会让渲染进程同时
 * 解码多张动图。这类页改成翻到时再加载。
 */
const PREFETCH_MAX_PAGE_BYTES = 12 * 1024 * 1024;
/** 一次预取最多提前准备多少字节。 */
const PREFETCH_BUDGET_BYTES = 48 * 1024 * 1024;
/** 预解码等待上限；超时说明这张图太大，交给 <img> 自己解码。 */
const PAGE_DECODE_TIMEOUT = 6000;
const ZOOM_MIN = 0.5;
const ZOOM_MAX = 4;
const ZOOM_STEP = 0.25;
const SPREAD_STORAGE_KEY = "archive-gallery:reader-spread";
const PLAYBACK_STORAGE_KEY = "archive-gallery:reader-playback";
/** 播放模式下静态页的停留时长。 */
const STATIC_PAGE_DWELL_MS = 2000;
/** 播放模式下动图页的停留上下限：太短看不清，太长（几分钟的长动图）会卡住模式。 */
const PLAYBACK_MIN_PAGE_MS = 400;
const PLAYBACK_MAX_PAGE_MS = 30000;
/** 统一帧播放器的单帧时长上下限：坏时长（0 或几十秒）不能把帧刷爆或长时间停住。 */
const FRAME_MIN_MS = 20;
const FRAME_MAX_MS = 4000;

function isAnimName(name: string): boolean {
  const extension = name.slice(name.lastIndexOf(".") + 1).toLowerCase();
  return extension === "gif" || extension === "webp";
}

function trimPageCache(cache: Map<number, ReaderPage>, current: number) {
  if (cache.size <= CACHE_LIMIT) return;

  for (const key of [...cache.keys()]) {
    if (cache.size <= CACHE_LIMIT) return;
    if (Math.abs(key - current) > 1) cache.delete(key);
  }

  for (const key of [...cache.keys()]) {
    if (cache.size <= CACHE_LIMIT) return;
    if (key !== current) cache.delete(key);
  }
}

const THUMB_EDGE = 160;

interface ReaderThumbProps {
  archiveId: number;
  index: number;
  active: boolean;
  cache: Map<number, string>;
  onSelect: (index: number) => void;
}

function ReaderThumbView({
  archiveId,
  index,
  active,
  cache,
  onSelect,
}: ReaderThumbProps) {
  const buttonRef = useRef<HTMLButtonElement | null>(null);
  const [visible, setVisible] = useState(false);
  const [src, setSrc] = useState<string | null>(
    () => cache.get(index) ?? null,
  );
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    if (visible) return;
    const node = buttonRef.current;
    if (!node) return;
    if (typeof IntersectionObserver === "undefined") {
      setVisible(true);
      return;
    }
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) {
          setVisible(true);
          observer.disconnect();
        }
      },
      {
        root: node.closest(".reader-thumbs-list"),
        rootMargin: "320px 0px",
      },
    );
    observer.observe(node);
    return () => observer.disconnect();
  }, [visible]);

  useEffect(() => {
    if (!visible || src) return;
    let cancelled = false;
    api
      .readReaderThumbnail(archiveId, index, THUMB_EDGE)
      .then((data) => {
        if (cancelled) return;
        cache.set(index, data.dataUrl);
        setSrc(data.dataUrl);
      })
      .catch(() => {
        if (!cancelled) setFailed(true);
      });
    return () => {
      cancelled = true;
    };
  }, [visible, src, archiveId, index, cache]);

  useEffect(() => {
    if (!active) return;
    buttonRef.current?.scrollIntoView({ block: "nearest" });
  }, [active]);

  return (
    <button
      ref={buttonRef}
      type="button"
      className={`reader-thumb${active ? " is-active" : ""}`}
      onClick={() => onSelect(index)}
      title={`第 ${index + 1} 页`}
      aria-current={active ? "page" : undefined}
    >
      <span className="reader-thumb-frame">
        {src ? (
          <img src={src} alt="" loading="lazy" draggable={false} />
        ) : failed ? (
          <span className="reader-thumb-failed">无法预览</span>
        ) : (
          <span className="spinner reader-thumb-spinner" />
        )}
      </span>
      <span className="reader-thumb-index">{index + 1}</span>
    </button>
  );
}

/**
 * 缩略图侧栏会把整本的每一页都挂出来，翻页时若整列重渲染，
 * 几百个按钮 + 交叉观察器一起抖动。props 全是标量或稳定引用，
 * 浅比较即可把开销压到「只有当前页那张变」。
 */
const ReaderThumb = memo(ReaderThumbView);

export default function Reader({
  card,
  onClose,
  onNotify,
  onProgress,
  onSwitchArchive,
  hasPrevArchive = false,
  hasNextArchive = false,
  onOpenDetail,
  detailOpen = false,
}: ReaderProps) {
  const [info, setInfo] = useState<ReaderInfo | null>(null);
  const [page, setPage] = useState(0);
  const [pageData, setPageData] = useState<ReaderPage | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [pageInput, setPageInput] = useState("1");
  const [zoomMode, setZoomMode] = useState<ZoomMode>("fit");
  const [zoom, setZoom] = useState(1);
  const [naturals, setNaturals] = useState<
    Record<number, { width: number; height: number }>
  >({});
  const [dragging, setDragging] = useState(false);
  const [reloadToken, setReloadToken] = useState(0);
  /**
   * 取图 URL 的版本号。只有重试时递增：换掉 URL 才能摆脱 webview 里
   * 已经记成「加载失败」的那张图（协议层缓存未命中时会返回 404）。
   */
  const [assetToken, setAssetToken] = useState(0);
  const [openToken, setOpenToken] = useState(0);
  const [spread, setSpread] = useState(() => {
    try {
      return window.localStorage.getItem(SPREAD_STORAGE_KEY) === "1";
    } catch {
      return false;
    }
  });
  const [secondData, setSecondData] = useState<ReaderPage | null>(null);
  const [bookmarks, setBookmarks] = useState<ReaderBookmark[]>([]);
  const [bookmarksOpen, setBookmarksOpen] = useState(false);
  const [bookmarkBusy, setBookmarkBusy] = useState(false);
  const [thumbsOpen, setThumbsOpen] = useState(false);
  /** 播放阅读模式（持久化）：开着的书播完自动翻页、翻完自动换下一本。 */
  const [playback, setPlayback] = useState(() => {
    try {
      return window.localStorage.getItem(PLAYBACK_STORAGE_KEY) === "1";
    } catch {
      return false;
    }
  });
  /** 播放是否正在进行（模式开着也可以用空格暂停）。 */
  const [playing, setPlaying] = useState(false);
  /** 当前本每页的动画元数据（帧数/总时长），按页号缓存。 */
  const [animMetas, setAnimMetas] = useState<Record<number, AnimMeta>>({});
  /** 统一帧播放器：当前显示的帧号（换页从 0 重来）。 */
  const [animFrame, setAnimFrame] = useState(0);
  /** 帧播放是否被用户暂停（拖帧 / 点暂停）；恢复后继续。 */
  const [framePaused, setFramePaused] = useState(false);
  /** 每页已解码就绪的帧数（顺序解码会话的进度）。 */
  const [framesReady, setFramesReady] = useState<Record<number, number>>({});

  const stageRef = useRef<HTMLDivElement | null>(null);
  const cacheRef = useRef<Map<number, ReaderPage>>(new Map());
  const thumbCacheRef = useRef<Map<number, string>>(new Map());
  const inflightRef = useRef<Map<number, Promise<ReaderPage>>>(new Map());
  const decodedRef = useRef<Map<number, Promise<void>>>(new Map());
  /** 每页每次打开只自动自愈一次，避免 404 → 重试 → 404 的死循环。 */
  const recoveredRef = useRef<Set<number>>(new Set());
  const requestSeqRef = useRef(0);
  const infoRef = useRef<ReaderInfo | null>(null);
  const pageRef = useRef(1);
  const onProgressRef = useRef(onProgress);
  onProgressRef.current = onProgress;
  const playingRef = useRef(playing);
  playingRef.current = playing;
  const playbackRef = useRef(playback);
  playbackRef.current = playback;
  const metaInflightRef = useRef<Set<number>>(new Set());
  /** 帧解码会话循环的去重状态：同一页只跑一条循环，target 只增不减。 */
  const framesLoopRef = useRef<{ page: number; target: number; running: boolean }>({
    page: -1,
    target: 0,
    running: false,
  });
  /** 拖帧条前的暂停状态：松手后按原样恢复，别把用户的暂停/播放意图改掉。 */
  const scrubPrevPausedRef = useRef(false);
  const panRef = useRef<{
    pointerId: number;
    startX: number;
    startY: number;
    left: number;
    top: number;
  } | null>(null);

  infoRef.current = info;
  pageRef.current = page + 1;

  const fetchPageData = useCallback(
    (index: number, current = false): Promise<ReaderPage> => {
      const cached = cacheRef.current.get(index);
      if (cached) return Promise.resolve(cached);
      const inflight = inflightRef.current.get(index);
      if (inflight) return inflight;

      const request = api
        .readReaderPage(card.id, index, current)
        .then((data) => {
          cacheRef.current.set(data.index, data);
          trimPageCache(cacheRef.current, pageRef.current - 1);
          return data;
        })
        .finally(() => {
          inflightRef.current.delete(index);
        });

      inflightRef.current.set(index, request);
      return request;
    },
    [card.id],
  );

  const preparePage = useCallback(
    (data: ReaderPage): Promise<void> => {
      const existing = decodedRef.current.get(data.index);
      if (existing) return existing;

      const promise = new Promise<void>((resolve) => {
        // 动图交给统一帧播放器：原图（动画 webp/gif）不再进 webview 解码。
        // 大动画在 Chromium 里解码经常失败或长时间不出图，正是「加载错误」
        // 与「进入后还在卡加载」的来源；帧的解码/播放由后端 + 播放器接管。
        if (isAnimName(data.name)) {
          resolve();
          return;
        }
        let settled = false;
        let timer = 0;
        // 预解码只是为了「翻到这一页时立刻有图」。静态页解码很快，超时只是保险。
        const bail = () => {
          if (settled) return;
          settled = true;
          window.clearTimeout(timer);
          resolve();
        };
        const finish = () => {
          if (settled) return;
          settled = true;
          window.clearTimeout(timer);
          if (typeof image.decode === "function") {
            image.decode().then(
              () => resolve(),
              () => resolve(),
            );
          } else {
            resolve();
          }
        };
        const image = new Image();
        image.decoding = "sync";
        image.onload = finish;
        image.onerror = () => {
          // 记成「已解码」会让这一页永远停在破图上：把记忆删掉，下次重来时
          // 还能真正重新预热一次。
          decodedRef.current.delete(data.index);
          bail();
        };
        image.src = readerPageUrl(
          card.id,
          data.index,
          assetToken,
          data.version,
        );
        if (image.complete) finish();
        timer = window.setTimeout(bail, PAGE_DECODE_TIMEOUT);
      });

      decodedRef.current.set(data.index, promise);
      return promise;
    },
    [card.id, assetToken],
  );

  /**
   * 顺序解码会话循环：把帧推进到目标帧。
   * 动画 gif/webp 无法随机访问帧（只能顺序解），单帧请求是 O(N)——
   * 拖条每格解一遍会把 CPU 打满。这里整页只跑一条循环：后端会话复用
   * 解码器增量推进（每帧只解一次，整体 O(target)），3 秒预算到就返回
   * 进度，ready 没到目标就自动续跑。
   */
  const ensureFramesTo = useCallback(
    (index: number, target: number) => {
      const loop = framesLoopRef.current;
      if (loop.page !== index) {
        loop.page = index;
        loop.target = 0;
      }
      if (target <= loop.target) return;
      loop.target = target;
      if (loop.running) return;
      loop.running = true;

      const step = async () => {
        const current = framesLoopRef.current;
        try {
          const progress = await api.prepareAnimFrames(
            card.id,
            current.page,
            current.target,
          );
          setFramesReady((previous) => ({
            ...previous,
            [current.page]: Math.max(progress.ready, previous[current.page] ?? 0),
          }));
          if (
            framesLoopRef.current.page === current.page &&
            framesLoopRef.current.target > progress.ready
          ) {
            void step();
            return;
          }
        } catch {
          // 解码失败静默：帧条仍可拖，未就绪时显示最近的一帧。
        }
        framesLoopRef.current.running = false;
      };
      void step();
    },
    [card.id],
  );

  const resetStageScroll = () => {
    const stage = stageRef.current;
    if (!stage) return;
    stage.scrollTop = 0;
    stage.scrollLeft = 0;
  };

  useEffect(() => {
    let cancelled = false;
    setInfo(null);
    setPageData(null);
    setError(null);
    setLoading(true);
    setNaturals({});
    setSecondData(null);
    setBookmarks([]);
    setBookmarksOpen(false);
    setThumbsOpen(false);
    setAnimMetas({});
    setFramesReady({});
    framesLoopRef.current = { page: -1, target: 0, running: false };
    // 换本（含播放模式跨本连播）保留当前播放状态；模式开着且没被暂停就继续播。
    setPlaying(playingRef.current);
    cacheRef.current.clear();
    thumbCacheRef.current.clear();
    inflightRef.current.clear();
    decodedRef.current.clear();
    metaInflightRef.current.clear();

    api
      .openReader(card.id)
      .then((next) => {
        if (cancelled) return;
        setInfo(next);
        setBookmarks(next.bookmarks ?? []);
        const reachedEnd = next.lastPage >= next.pageCount && next.pageCount > 0;
        const start = reachedEnd
          ? 0
          : next.pageCount > 0
            ? Math.min(Math.max(next.lastPage, 1), next.pageCount) - 1
            : 0;
        setPage(start);
        setPageInput(String(start + 1));
        if (reachedEnd) {
          onNotify("这本已经读完，从头开始", "info");
        } else if (next.lastPage > 1 && next.lastPage <= next.pageCount) {
          onNotify(`已从上次第 ${next.lastPage} 页继续`, "info");
        }
      })
      .catch((openError) => {
        if (cancelled) return;
        setError(String(openError));
        setLoading(false);
      });

    return () => {
      cancelled = true;
    };
  }, [card.id, openToken, onNotify]);

  useEffect(() => {
    if (!info || info.pageCount === 0) return;

    const seq = ++requestSeqRef.current;
    setPageInput(String(page + 1));

    const secondIndex = spread && page + 1 < info.pageCount ? page + 1 : null;
    const cached = cacheRef.current.get(page);
    const cachedSecond =
      secondIndex !== null ? (cacheRef.current.get(secondIndex) ?? null) : null;

    if (cached && (secondIndex === null || cachedSecond)) {
      Promise.all([
        preparePage(cached),
        cachedSecond ? preparePage(cachedSecond) : Promise.resolve(),
      ]).then(() => {
        if (seq !== requestSeqRef.current) return;
        resetStageScroll();
        setPageData(cached);
        setSecondData(cachedSecond);
        setError(null);
        setLoading(false);
      });
      return;
    }

    setLoading(true);
    setError(null);
    Promise.all([
      fetchPageData(page, true),
      secondIndex !== null
        ? fetchPageData(secondIndex, true)
        : Promise.resolve(null),
    ])
      .then(async ([first, second]) => {
        await Promise.all([
          preparePage(first),
          second ? preparePage(second) : Promise.resolve(),
        ]);
        if (seq !== requestSeqRef.current) return;
        resetStageScroll();
        setPageData(first);
        setSecondData(second);
        setLoading(false);
      })
      .catch((pageError) => {
        if (seq !== requestSeqRef.current) return;
        setError(String(pageError));
        setLoading(false);
      });
  }, [info, page, spread, card.id, reloadToken, fetchPageData, preparePage]);

  useEffect(() => {
    try {
      window.localStorage.setItem(SPREAD_STORAGE_KEY, spread ? "1" : "0");
    } catch {
      // Local storage may be unavailable; the mode still works for this session.
    }
  }, [spread]);

  useEffect(() => {
    try {
      window.localStorage.setItem(PLAYBACK_STORAGE_KEY, playback ? "1" : "0");
    } catch {
      // Local storage may be unavailable; the mode still works for this session.
    }
    // 模式开关直接驱动播放：打开就播，关掉就停。
    setPlaying(playback);
  }, [playback]);

  // 当前页是动图时拉取帧元数据（帧数 / 总时长），拖条与播放模式都要用。
  useEffect(() => {
    if (!pageData || !isAnimName(pageData.name)) return;
    const index = pageData.index;
    if (animMetas[index] || metaInflightRef.current.has(index)) return;

    metaInflightRef.current.add(index);
    let cancelled = false;
    api
      .readAnimMeta(card.id, index)
      .then((meta) => {
        if (cancelled) return;
        setAnimMetas((previous) => ({ ...previous, [index]: meta }));
        // 动图页天然尺寸用元数据里的原始尺寸：帧可能被后端降采样过，别用帧的尺寸。
        if (meta.animated && meta.width > 0) {
          setNaturals((previous) => ({
            ...previous,
            [index]: { width: meta.width, height: meta.height },
          }));
        }
      })
      .catch((error) => {
        // 拿不到元数据就当作静态页记下来：不出拖条，播放按停留时长走，
        // 否则这一页会永远等不到时长、播放模式卡死在当前页。
        // 失败原因必须浮出来：静默吞掉会让「动图没被识别」无从排查。
        if (!cancelled) {
          setAnimMetas((previous) => ({
            ...previous,
            [index]: {
              animated: false,
              frames: 0,
              totalDurationMs: 0,
              frameDurationsMs: [],
              width: 0,
              height: 0,
            },
          }));
          onNotify(`动画信息读取失败，本页按静态页处理：${String(error)}`, "error");
        }
      })
      .finally(() => {
        metaInflightRef.current.delete(index);
      });
    return () => {
      cancelled = true;
    };
  }, [pageData, card.id, animMetas, onNotify]);

  // —— 统一帧播放器（动图）——
  // 动图页不再让 webview 直接解原图，改由后端把整段帧解成 PNG/JPEG，前端按
  // 真实帧时长自绘播放。这样既能规避大动画在 Chromium 里的解码失败，也让
  // 「擦洗 / 暂停 / 逐帧」真正可控。
  const currentMeta = pageData ? animMetas[pageData.index] : undefined;
  const isAnimated = !!currentMeta?.animated && currentMeta.frames > 1;
  const readyFrames = pageData ? framesReady[pageData.index] ?? 0 : 0;
  /** 只显示已解出的帧里最接近目标的那一帧，避免 <img> 请求到还没解出的帧。 */
  const displayFrame = isAnimated
    ? Math.min(animFrame, Math.max(readyFrames - 1, 0))
    : 0;
  const frameSrc =
    pageData && isAnimated && readyFrames > 0
      ? animFrameUrl(
          card.id,
          pageData.index,
          displayFrame,
          assetToken,
          pageData.version,
        )
      : null;
  /** 主画面的 URL：动图用帧，其余用原图。动图未就绪时为 null（走加载态）。 */
  const mainSrc = pageData
    ? isAnimated
      ? frameSrc
      : readerPageUrl(card.id, pageData.index, assetToken, pageData.version)
    : null;

  // 换页：帧播放从头开始，并清掉上一页的暂停状态。
  useEffect(() => {
    setAnimFrame(0);
    setFramePaused(false);
  }, [pageData?.index]);

  // 动图页一成当前页就把整段帧解出来（顺序会话，3 秒一档自动续跑），
  // 「进入后还在卡加载」的窗口因此很短，帧条 / 播放也立刻可控。
  useEffect(() => {
    if (!pageData || !isAnimated || !currentMeta) return;
    ensureFramesTo(pageData.index, currentMeta.frames - 1);
  }, [pageData, isAnimated, currentMeta, ensureFramesTo]);

  // 帧节拍：按逐帧真实时长推进。普通阅读时循环播放；播放模式下只播一遍，
  // 播完停在最后一帧，交给翻页节拍换页。
  useEffect(() => {
    if (!pageData || !isAnimated || !currentMeta || framePaused) return;
    const frames = currentMeta.frames;
    if (readyFrames <= animFrame) return; // 当前帧还没解出来：等 ready 变化重跑
    const current = Math.min(animFrame, frames - 1);
    const raw = currentMeta.frameDurationsMs[current] ?? 100;
    const delay = Math.min(Math.max(raw, FRAME_MIN_MS), FRAME_MAX_MS);
    const isLast = current >= frames - 1;
    const looping = !(playback && playing);
    if (isLast && !looping) return; // 播放模式：停在最后一帧
    const timer = window.setTimeout(() => {
      setAnimFrame(isLast ? 0 : current + 1);
    }, delay);
    return () => window.clearTimeout(timer);
  }, [
    pageData,
    isAnimated,
    currentMeta,
    framePaused,
    readyFrames,
    animFrame,
    playback,
    playing,
  ]);

  // 播放模式节拍：动图按真实总时长播一遍再翻页，静态页停 2 秒；
  // 翻到本尾自动换下一本，没有下一本就结束播放。
  useEffect(() => {
    if (!playing || !info || info.pageCount === 0 || !pageData) return;
    const looksAnim = isAnimName(pageData.name);
    const meta = animMetas[pageData.index];
    // 动图页等元数据到位、且至少解出第一帧再计时：否则长动图会被当成
    // 静态页 2 秒翻走，或翻页时动画还没开始播。
    if (looksAnim && (!meta || (meta.animated && meta.frames > 1 && (framesReady[pageData.index] ?? 0) === 0))) {
      return;
    }

    const duration =
      meta?.animated && meta.totalDurationMs > 0
        ? Math.min(Math.max(meta.totalDurationMs, PLAYBACK_MIN_PAGE_MS), PLAYBACK_MAX_PAGE_MS)
        : STATIC_PAGE_DWELL_MS;

    const timer = window.setTimeout(() => {
      const count = info.pageCount;
      const nextIndex = page + (spread ? 2 : 1);
      if (nextIndex < count) {
        setPage(nextIndex);
        return;
      }
      if (onSwitchArchive && hasNextArchive) {
        onSwitchArchive(1);
        return;
      }
      setPlaying(false);
      onNotify("已经到最后一页，播放结束", "info");
    }, duration);
    return () => window.clearTimeout(timer);
  }, [
    playing,
    info,
    pageData,
    animMetas,
    framesReady,
    page,
    spread,
    hasNextArchive,
    onSwitchArchive,
    onNotify,
  ]);

  // 换页后重新给「自愈」机会：上一次失败的页再回来时可以再试一次。
  useEffect(() => {
    recoveredRef.current.clear();
  }, [page, card.id]);

  useEffect(() => {
    if (!info || info.pageCount === 0) return;

    const step = spread ? 2 : 1;
    const indexes: number[] = [];
    for (let i = 1; i <= PREFETCH_AHEAD; i += 1) {
      const forward = page + i * step;
      if (forward < info.pageCount) indexes.push(forward);
    }
    for (let i = 1; i <= PREFETCH_BEHIND; i += 1) {
      const backward = page - i * step;
      if (backward >= 0) indexes.push(backward);
    }

    let cancelled = false;
    const run = async () => {
      let spent = 0;
      for (const index of indexes) {
        if (cancelled) return;
        try {
          const data =
            cacheRef.current.get(index) ?? (await fetchPageData(index));
          if (cancelled) return;

          // 大页（动图）不预取：一是几十兆一份，预取 4 页就超过后端 96MB
          // 缓存预算，会反过来把正在看的那页挤出去；二是渲染进程解码多张
          // 动图本身就可能失败。留到用户真的翻过去时再加载。
          if (data.bytes > PREFETCH_MAX_PAGE_BYTES) continue;
          if (spent + data.bytes > PREFETCH_BUDGET_BYTES) return;
          spent += data.bytes;

          await preparePage(data);
        } catch {
          // Prefetch failures must never disturb the frame on screen.
        }
      }
    };
    void run();

    return () => {
      cancelled = true;
    };
  }, [info, page, spread, fetchPageData, preparePage]);

  useEffect(() => {
    if (!info) return;
    const timer = window.setTimeout(() => {
      void api
        .setReadingProgress(card.id, page + 1)
        .then(() => onProgressRef.current?.(card.id, page + 1))
        .catch(() => {});
    }, 700);
    return () => window.clearTimeout(timer);
  }, [page, info, card.id]);

  useEffect(() => {
    return () => {
      if (infoRef.current && infoRef.current.pageCount > 0) {
        const lastPage = pageRef.current;
        void api
          .setReadingProgress(card.id, lastPage)
          .then(() => onProgressRef.current?.(card.id, lastPage))
          .catch(() => {});
      }
    };
  }, [card.id]);

  const goTo = useCallback(
    (index: number) => {
      const count = info?.pageCount ?? 0;
      if (count === 0) return;
      setPage(Math.min(Math.max(index, 0), count - 1));
    },
    [info],
  );

  const step = spread ? 2 : 1;
  const goNext = useCallback(() => goTo(page + step), [goTo, page, step]);
  const goPrev = useCallback(() => goTo(page - step), [goTo, page, step]);

  /** 任何手动操作（翻页/缩放/拖动/跳页）都暂停播放，交给用户接管。 */
  const pausePlayback = useCallback(() => {
    setPlaying(false);
  }, []);

  const togglePlayback = useCallback(() => {
    setPlayback((value) => !value);
  }, []);

  /** 拖帧条：跳到该帧；拖动期间暂停自动播放，松手按原状态恢复。 */
  const beginScrub = () => {
    scrubPrevPausedRef.current = framePaused;
    setFramePaused(true);
  };

  const scrubToFrame = (value: number) => {
    if (!pageData) return;
    setAnimFrame(value);
    ensureFramesTo(pageData.index, value);
  };

  const endScrub = () => {
    setFramePaused(scrubPrevPausedRef.current);
  };

  /** 逐帧前进 / 后退并暂停自动播放，方便定格看细节。 */
  const stepFrame = (delta: number) => {
    if (!pageData || !currentMeta) return;
    const frames = currentMeta.frames;
    setFramePaused(true);
    setAnimFrame((value) => Math.min(Math.max(value + delta, 0), frames - 1));
  };

  const toggleSpread = useCallback(() => {
    setSpread((value) => !value);
  }, []);

  const currentPage = page + 1;
  const currentBookmark = bookmarks.find(
    (bookmark) => bookmark.page === currentPage,
  );

  const toggleBookmark = useCallback(async () => {
    if (!info || bookmarkBusy) return;
    setBookmarkBusy(true);
    try {
      if (currentBookmark) {
        await api.removeReaderBookmark(card.id, currentBookmark.id);
        setBookmarks((previous) =>
          previous.filter((bookmark) => bookmark.id !== currentBookmark.id),
        );
        onNotify(`已移除第 ${currentPage} 页书签`, "info");
      } else {
        const created = await api.addReaderBookmark(card.id, currentPage);
        setBookmarks((previous) =>
          [
            ...previous.filter((bookmark) => bookmark.page !== created.page),
            created,
          ].sort((left, right) => left.page - right.page),
        );
        onNotify(`已收藏第 ${currentPage} 页`, "success");
      }
    } catch (bookmarkError) {
      onNotify(`书签操作失败：${String(bookmarkError)}`, "error");
    } finally {
      setBookmarkBusy(false);
    }
  }, [info, bookmarkBusy, currentBookmark, card.id, currentPage, onNotify]);

  const removeBookmark = useCallback(
    async (bookmark: ReaderBookmark) => {
      setBookmarkBusy(true);
      try {
        await api.removeReaderBookmark(card.id, bookmark.id);
        setBookmarks((previous) =>
          previous.filter((item) => item.id !== bookmark.id),
        );
        onNotify(`已移除第 ${bookmark.page} 页书签`, "info");
      } catch (bookmarkError) {
        onNotify(`删除书签失败：${String(bookmarkError)}`, "error");
      } finally {
        setBookmarkBusy(false);
      }
    },
    [card.id, onNotify],
  );

  const zoomIn = useCallback(() => {
    if (zoomMode !== "actual") {
      setZoomMode("actual");
      setZoom(1);
      return;
    }
    setZoom((value) =>
      Math.min(ZOOM_MAX, Math.round((value + ZOOM_STEP) * 100) / 100),
    );
  }, [zoomMode]);

  const zoomOut = useCallback(() => {
    if (zoomMode !== "actual") {
      setZoomMode("actual");
      setZoom(0.75);
      return;
    }
    setZoom((value) =>
      Math.max(ZOOM_MIN, Math.round((value - ZOOM_STEP) * 100) / 100),
    );
  }, [zoomMode]);

  useEffect(() => {
    const handleKey = (event: KeyboardEvent) => {
      // 详情浮层开着时按键全部让给浮层（它自己处理 Esc 关闭）。
      if (detailOpen) return;

      if (event.key === "Escape") {
        event.preventDefault();
        onClose();
        return;
      }

      const target = event.target as HTMLElement | null;
      if (
        target &&
        (target.tagName === "INPUT" ||
          target.tagName === "TEXTAREA" ||
          target.tagName === "BUTTON" ||
          target.tagName === "SELECT" ||
          target.tagName === "A" ||
          target.isContentEditable ||
          // 焦点在能自己响应空格的控件上时让出原生行为：
          // 否则空格既翻页又吃掉按钮激活，键盘用户没法用空格按按钮。
          target.closest("[role='button'], [role='link']") !== null)
      ) {
        return;
      }

      // 换本键放在 !info 之前：本子打不开时也该能跳去下一本。
      if (onSwitchArchive && event.key === "[") {
        event.preventDefault();
        pausePlayback();
        onSwitchArchive(-1);
        return;
      }
      if (onSwitchArchive && event.key === "]") {
        event.preventDefault();
        pausePlayback();
        onSwitchArchive(1);
        return;
      }

      if (!info) return;
      const last = info.pageCount - 1;
      let handled = true;

      if (event.key.toLowerCase() === "p") {
        togglePlayback();
      } else if (onOpenDetail && event.key.toLowerCase() === "i") {
        onOpenDetail();
      } else if (
        event.key === "ArrowRight" ||
        event.key === "ArrowDown" ||
        event.key === "PageDown" ||
        (event.key === " " && !event.shiftKey)
      ) {
        if (event.key === " " && playback) {
          // 播放模式下空格 = 暂停/继续，不再当翻页键。
          setPlaying((value) => !value);
        } else {
          pausePlayback();
          goNext();
        }
      } else if (
        event.key === "ArrowLeft" ||
        event.key === "ArrowUp" ||
        event.key === "PageUp" ||
        (event.key === " " && event.shiftKey)
      ) {
        pausePlayback();
        goPrev();
      } else if (event.key === "Home") {
        pausePlayback();
        goTo(0);
      } else if (event.key === "End") {
        pausePlayback();
        goTo(last);
      } else if (event.key === "+" || event.key === "=") {
        pausePlayback();
        zoomIn();
      } else if (event.key === "-") {
        pausePlayback();
        zoomOut();
      } else if (event.key === "0") {
        setZoomMode("fit");
      } else if (event.key.toLowerCase() === "d") {
        toggleSpread();
      } else if (event.key.toLowerCase() === "b") {
        void toggleBookmark();
      } else if (event.key.toLowerCase() === "l") {
        setBookmarksOpen((value) => !value);
      } else if (event.key.toLowerCase() === "t") {
        setThumbsOpen((value) => !value);
      } else {
        handled = false;
      }

      if (handled) event.preventDefault();
    };

    window.addEventListener("keydown", handleKey);
    return () => window.removeEventListener("keydown", handleKey);
  }, [
    info,
    goTo,
    goNext,
    goPrev,
    onClose,
    zoomIn,
    zoomOut,
    toggleSpread,
    toggleBookmark,
    onSwitchArchive,
    detailOpen,
    playback,
    pausePlayback,
    togglePlayback,
    onOpenDetail,
  ]);

  useEffect(() => {
    const stage = stageRef.current;
    if (!stage) return;

    let accumulated = 0;
    let lastWheelAt = 0;
    let lastFlipAt = 0;

    const handleWheel = (event: WheelEvent) => {
      if (event.ctrlKey || event.metaKey) {
        event.preventDefault();
        pausePlayback();
        if (event.deltaY < 0) zoomIn();
        else zoomOut();
        return;
      }

      const unit =
        event.deltaMode === 1
          ? 16
          : event.deltaMode === 2
            ? stage.clientHeight
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

      // Zoomed-in pages stay pannable: keep native scrolling until an edge.
      if (horizontal) {
        const max = stage.scrollWidth - stage.clientWidth;
        if (
          max > 1 &&
          ((delta < 0 && stage.scrollLeft > 0) ||
            (delta > 0 && stage.scrollLeft < max - 1))
        ) {
          return;
        }
      } else {
        const max = stage.scrollHeight - stage.clientHeight;
        if (
          max > 1 &&
          ((delta < 0 && stage.scrollTop > 0) ||
            (delta > 0 && stage.scrollTop < max - 1))
        ) {
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
      if (Math.abs(accumulated) < 40) return;

      const direction = accumulated > 0 ? 1 : -1;
      accumulated = 0;
      lastFlipAt = now;
      pausePlayback();
      if (direction > 0) goNext();
      else goPrev();
    };

    stage.addEventListener("wheel", handleWheel, { passive: false });
    return () => stage.removeEventListener("wheel", handleWheel);
  }, [goNext, goPrev, zoomIn, zoomOut, pausePlayback]);

  const commitPageInput = useCallback(() => {
    const count = info?.pageCount ?? 0;
    if (count === 0) return;
    const parsed = Number.parseInt(pageInput, 10);
    if (Number.isNaN(parsed)) {
      setPageInput(String(page + 1));
      return;
    }
    pausePlayback();
    goTo(parsed - 1);
    setPageInput(String(Math.min(Math.max(parsed, 1), count)));
  }, [info, pageInput, page, goTo, pausePlayback]);

  const retry = () => {
    if (!info) {
      setOpenToken((value) => value + 1);
      return;
    }
    // 只作废当前这一页（双页时连带第二页），别把已经预热好的邻页一起清掉：
    // 清整张表会让重试变成「把几十兆重新解压一遍」。
    for (const index of [page, page + 1]) {
      cacheRef.current.delete(index);
      inflightRef.current.delete(index);
      decodedRef.current.delete(index);
    }
    // URL 里带 token，换 token 后旧的解码结果全部作废。
    setAssetToken((value) => value + 1);
    setReloadToken((value) => value + 1);
  };

  /**
   * 图片加载失败时先自愈一次。
   *
   * 协议层只在后端缓存未命中时返回 404（例如页面被 LRU 淘汰），这时重挂一次
   * <img>（换 URL 版本号）通常就能拿到图：后端缓存是热的，命令层不必重跑。
   * 同一次打开里每页只自愈一次，再失败就走错误态，交给用户点「重试」。
   */
  const handleImageError = (index: number) => {
    if (recoveredRef.current.has(index)) {
      setLoading(false);
      setError("页面图片加载失败，请重试");
      return;
    }
    recoveredRef.current.add(index);
    // 只换 <img> 的 key 与 URL 版本，不动后端缓存、不重新走命令。
    decodedRef.current.delete(index);
    setAssetToken((value) => value + 1);
  };

  const handleImageLoad = useCallback(
    (index: number, event: ReactSyntheticEvent<HTMLImageElement>) => {
      const { naturalWidth, naturalHeight } = event.currentTarget;
      setNaturals((previous) => ({
        ...previous,
        [index]: { width: naturalWidth, height: naturalHeight },
      }));
    },
    [],
  );

  const handlePointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    const stage = stageRef.current;
    if (!stage || event.button !== 0) return;
    if (
      stage.scrollWidth <= stage.clientWidth &&
      stage.scrollHeight <= stage.clientHeight
    ) {
      return;
    }
    pausePlayback();
    panRef.current = {
      pointerId: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      left: stage.scrollLeft,
      top: stage.scrollTop,
    };
    stage.setPointerCapture(event.pointerId);
    setDragging(true);
  };

  const handlePointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    const stage = stageRef.current;
    const pan = panRef.current;
    if (!stage || !pan || pan.pointerId !== event.pointerId) return;
    stage.scrollLeft = pan.left - (event.clientX - pan.startX);
    stage.scrollTop = pan.top - (event.clientY - pan.startY);
  };

  const handlePointerUp = (event: ReactPointerEvent<HTMLDivElement>) => {
    const stage = stageRef.current;
    if (stage && panRef.current?.pointerId === event.pointerId) {
      stage.releasePointerCapture(event.pointerId);
    }
    panRef.current = null;
    setDragging(false);
  };

  const pageCount = info?.pageCount ?? 0;
  const lastVisible =
    spread && page + 1 < pageCount ? page + 1 : page;
  const visibleSecond =
    pageData && secondData && secondData.index === pageData.index + 1
      ? secondData
      : null;
  const currentName =
    pageData && pageData.index === page
      ? visibleSecond
        ? `${pageData.name} + ${visibleSecond.name}`
        : pageData.name
      : "正在读取…";
  const pageLabel =
    pageCount > 0
      ? spread && page + 1 < pageCount
        ? `${page + 1}–${page + 2} / ${pageCount}`
        : `${page + 1} / ${pageCount}`
      : "—";
  const progress =
    pageCount > 0 ? Math.min(100, ((lastVisible + 1) / pageCount) * 100) : 0;
  const canNext = pageCount > 0 && page + step < pageCount;
  const zoomLabel =
    zoomMode === "fit"
      ? "适应页面"
      : zoomMode === "width"
        ? "适应宽度"
        : `${Math.round(zoom * 100)}%`;

  const imageStyleFor = (data: ReaderPage) => {
    const natural = naturals[data.index];
    return zoomMode === "actual" && natural
      ? { width: natural.width * zoom, height: natural.height * zoom }
      : undefined;
  };

  return (
    <div
      className="reader"
      role="dialog"
      aria-modal="true"
      aria-label={`阅读 ${card.title}`}
    >
      <header className="reader-head">
        <div className="reader-head-title">
          <span className="reader-icon">
            <BookIcon width={16} height={16} />
          </span>
          <div>
            <p className="eyebrow">READER</p>
            <h2 title={card.title}>{card.title}</h2>
          </div>
        </div>
        <div className="reader-head-meta">
          <span className="reader-page-counter">
            {pageCount > 0 ? pageLabel : "—"}
          </span>
          <span className="reader-page-name" title={pageData?.name ?? ""}>
            {currentName}
          </span>
          <div className="reader-head-tools">
            {onSwitchArchive && (
              <>
                <button
                  type="button"
                  className="icon-button"
                  onClick={() => onSwitchArchive(-1)}
                  disabled={!hasPrevArchive}
                  aria-label="上一本画集"
                  title="上一本 ([)"
                >
                  <ChevronLeftIcon width={16} height={16} />
                </button>
                <button
                  type="button"
                  className="icon-button"
                  onClick={() => onSwitchArchive(1)}
                  disabled={!hasNextArchive}
                  aria-label="下一本画集"
                  title="下一本 (])"
                >
                  <ChevronRightIcon width={16} height={16} />
                </button>
              </>
            )}
            <button
              type="button"
              className={`icon-button${thumbsOpen ? " is-active" : ""}`}
              onClick={() => setThumbsOpen((value) => !value)}
              disabled={pageCount === 0}
              aria-label="页面缩略图"
              title="页面缩略图 (T)"
            >
              <GridIcon width={16} height={16} />
            </button>
            <button
              type="button"
              className={`icon-button${currentBookmark ? " is-active" : ""}`}
              onClick={() => void toggleBookmark()}
              disabled={bookmarkBusy || pageCount === 0}
              aria-label={currentBookmark ? "移除当前页书签" : "收藏当前页"}
              title={currentBookmark ? "移除当前页书签 (B)" : "收藏当前页 (B)"}
            >
              <StarIcon width={16} height={16} />
            </button>
            <button
              type="button"
              className={`icon-button${bookmarksOpen ? " is-active" : ""}`}
              onClick={() => setBookmarksOpen((value) => !value)}
              aria-label="书签列表"
              title="书签列表 (L)"
            >
              <BookIcon width={16} height={16} />
              {bookmarks.length > 0 && (
                <span className="reader-bookmark-count">{bookmarks.length}</span>
              )}
            </button>
            {onOpenDetail && (
              <button
                type="button"
                className="icon-button"
                onClick={onOpenDetail}
                aria-label="详情与编辑"
                title="详情与编辑 (I)"
              >
                <TagIcon width={16} height={16} />
              </button>
            )}
            <button
              type="button"
              className="icon-button"
              onClick={onClose}
              aria-label="关闭阅读器"
            >
              <CloseIcon width={16} height={16} />
            </button>
          </div>
        </div>
      </header>

      {bookmarksOpen && (
        <aside className="reader-bookmarks" aria-label="书签列表">
          <header>
            <h3>书签</h3>
            <button
              type="button"
              className="icon-button"
              onClick={() => setBookmarksOpen(false)}
              aria-label="关闭书签列表"
            >
              <CloseIcon width={14} height={14} />
            </button>
          </header>
          {bookmarks.length === 0 ? (
            <p className="reader-bookmark-empty">
              还没有书签。按 B 或点击星标收藏当前页。
            </p>
          ) : (
            <ul>
              {bookmarks.map((bookmark) => (
                <li
                  key={bookmark.id}
                  className={bookmark.page === currentPage ? "is-current" : ""}
                >
                  <button
                    type="button"
                    className="reader-bookmark-jump"
                    onClick={() => {
                      pausePlayback();
                      goTo(bookmark.page - 1);
                    }}
                  >
                    第 {bookmark.page} 页
                  </button>
                  <button
                    type="button"
                    className="reader-bookmark-remove"
                    onClick={() => void removeBookmark(bookmark)}
                    disabled={bookmarkBusy}
                    aria-label={`删除第 ${bookmark.page} 页书签`}
                  >
                    <TrashIcon width={13} height={13} />
                  </button>
                </li>
              ))}
            </ul>
          )}
        </aside>
      )}

      <div className="reader-progress" aria-hidden="true">
        <span style={{ width: `${progress}%` }} />
      </div>

      <div className="reader-body">
        {thumbsOpen && pageCount > 0 && (
          <aside className="reader-thumbs" aria-label="页面缩略图">
            <header>
              <h3>页面</h3>
              <button
                type="button"
                className="icon-button"
                onClick={() => setThumbsOpen(false)}
                aria-label="关闭缩略图侧栏"
                title="关闭缩略图侧栏 (T)"
              >
                <CloseIcon width={14} height={14} />
              </button>
            </header>
            <div className="reader-thumbs-list">
              {Array.from({ length: pageCount }, (_, index) => (
                <ReaderThumb
                  key={index}
                  archiveId={card.id}
                  index={index}
                  active={index === page || (spread && index === page + 1)}
                  cache={thumbCacheRef.current}
                  onSelect={(index) => {
                    pausePlayback();
                    goTo(index);
                  }}
                />
              ))}
            </div>
          </aside>
        )}

        <div
          ref={stageRef}
          className={`reader-stage is-${zoomMode}${spread ? " is-spread" : ""}${dragging ? " is-dragging" : ""}`}
          onPointerDown={handlePointerDown}
          onPointerMove={handlePointerMove}
          onPointerUp={handlePointerUp}
          onPointerCancel={handlePointerUp}
          onDoubleClick={() => {
            if (zoomMode === "fit") {
              setZoomMode("actual");
              setZoom(1);
            } else {
              setZoomMode("fit");
            }
          }}
        >
          {pageData && mainSrc &&
            (spread ? (
              <div className="reader-spread">
                <img
                  key={`${pageData.index}-${isAnimated ? `f${displayFrame}` : assetToken}`}
                  src={mainSrc}
                  alt={pageData.name}
                  style={imageStyleFor(pageData)}
                  decoding="sync"
                  draggable={false}
                  onLoad={(event) => handleImageLoad(pageData.index, event)}
                  onError={() => {
                    // 动图帧未就绪时 404 属正常（会话还在解码），不要触发
                    // 页面级自愈/错误态——那会把整页切到「加载失败」。
                    if (isAnimated) return;
                    handleImageError(pageData.index);
                  }}
                />
                {visibleSecond && (
                  <img
                    key={`${visibleSecond.index}-${assetToken}`}
                    src={readerPageUrl(
                      card.id,
                      visibleSecond.index,
                      assetToken,
                      visibleSecond.version,
                    )}
                    alt={visibleSecond.name}
                    style={imageStyleFor(visibleSecond)}
                    decoding="sync"
                    draggable={false}
                    onLoad={(event) =>
                      handleImageLoad(visibleSecond.index, event)
                    }
                    onError={() => {
                      // 双页里第二页若是动图，原图仍由 webview 解，可能解码失败；
                      // 这里不让它把整页拖进「加载失败」态。
                      if (isAnimName(visibleSecond.name)) return;
                      handleImageError(visibleSecond.index);
                    }}
                  />
                )}
              </div>
            ) : (
              <img
                key={`${pageData.index}-${isAnimated ? `f${displayFrame}` : assetToken}`}
                src={mainSrc}
                alt={pageData.name}
                style={imageStyleFor(pageData)}
                decoding="sync"
                draggable={false}
                onLoad={(event) => handleImageLoad(pageData.index, event)}
                onError={() => {
                  if (isAnimated) return;
                  handleImageError(pageData.index);
                }}
              />
            ))}

          {pageData && !mainSrc && !error && (
            <div className="reader-state">
              <span className="spinner" />
              <p>正在解码动画帧…</p>
            </div>
          )}

          {!pageData && loading && (
            <div className="reader-state">
              <span className="spinner" />
              <p>
                {info ? `正在读取第 ${page + 1} 页…` : "正在打开压缩包…"}
              </p>
            </div>
          )}

          {error && (
            <div className="reader-state is-error">
              <AlertIcon width={22} height={22} />
              <p>{error}</p>
              <button
                type="button"
                className="btn btn-ghost btn-compact"
                onClick={retry}
              >
                重试
              </button>
            </div>
          )}

          {pageData && loading && (
            <span className="reader-busy" aria-hidden="true">
              <span className="spinner" />
            </span>
          )}
        </div>
      </div>

      <footer className="reader-bar">
        <div className="reader-nav">
          {onSwitchArchive && (
            <button
              type="button"
              className="btn btn-ghost btn-compact"
              disabled={!hasPrevArchive}
              onClick={() => onSwitchArchive(-1)}
              title="上一本画集 ([)"
            >
              <ChevronLeftIcon width={14} height={14} />
              上一本
            </button>
          )}
          <button
            type="button"
            className="btn btn-ghost btn-compact"
            disabled={page <= 0}
            onClick={goPrev}
          >
            <ChevronLeftIcon width={14} height={14} />
            上一页
          </button>
          <label className="reader-page-jump">
            第
            <input
              value={pageInput}
              inputMode="numeric"
              aria-label="跳转页码"
              onChange={(event) =>
                setPageInput(event.target.value.replace(/[^\d]/g, ""))
              }
              onBlur={commitPageInput}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  event.preventDefault();
                  commitPageInput();
                  event.currentTarget.blur();
                } else if (event.key === "Escape") {
                  event.preventDefault();
                  event.stopPropagation();
                  setPageInput(String(page + 1));
                  event.currentTarget.blur();
                }
              }}
            />
            页
            <span>/ {pageCount || "—"}</span>
          </label>
          <button
            type="button"
            className="btn btn-ghost btn-compact"
            disabled={!canNext}
            onClick={goNext}
          >
            下一页
            <ChevronRightIcon width={14} height={14} />
          </button>
          {onSwitchArchive && (
            <button
              type="button"
              className="btn btn-ghost btn-compact"
              disabled={!hasNextArchive}
              onClick={() => onSwitchArchive(1)}
              title="下一本画集 (])"
            >
              下一本
              <ChevronRightIcon width={14} height={14} />
            </button>
          )}
        </div>

        {isAnimated && currentMeta && (
          <div className="reader-frame-scrub">
            <button
              type="button"
              className="icon-button"
              onClick={() => setFramePaused((value) => !value)}
              aria-label={framePaused ? "继续播放动画" : "暂停动画"}
              title={framePaused ? "继续播放动画" : "暂停动画"}
            >
              {framePaused ? (
                <PlayIcon width={12} height={12} />
              ) : (
                <PauseIcon width={12} height={12} />
              )}
            </button>
            <button
              type="button"
              className="icon-button"
              onClick={() => stepFrame(-1)}
              disabled={displayFrame <= 0}
              aria-label="上一帧"
              title="上一帧"
            >
              <ChevronLeftIcon width={12} height={12} />
            </button>
            <button
              type="button"
              className="icon-button"
              onClick={() => stepFrame(1)}
              disabled={displayFrame >= currentMeta.frames - 1}
              aria-label="下一帧"
              title="下一帧"
            >
              <ChevronRightIcon width={12} height={12} />
            </button>
            <span className="reader-frame-label">
              帧 {displayFrame + 1} / {currentMeta.frames}
              {readyFrames < currentMeta.frames
                ? ` · 解码中 ${readyFrames}/${currentMeta.frames}`
                : ""}
            </span>
            <input
              type="range"
              min={0}
              max={currentMeta.frames - 1}
              step={1}
              value={displayFrame}
              onPointerDown={beginScrub}
              onChange={(event) => scrubToFrame(Number(event.target.value))}
              onPointerUp={endScrub}
              onBlur={endScrub}
              aria-label="动画帧擦洗"
            />
          </div>
        )}

        <div className="reader-zoom">
          <button
            type="button"
            className="reader-zoom-step"
            onClick={zoomOut}
            aria-label="缩小"
            title="缩小"
          >
            −
          </button>
          <span className="reader-zoom-label">{zoomLabel}</span>
          <button
            type="button"
            className="reader-zoom-step"
            onClick={zoomIn}
            aria-label="放大"
            title="放大"
          >
            +
          </button>
          <div className="reader-zoom-presets">
            <button
              type="button"
              className={zoomMode === "fit" ? "is-active" : ""}
              onClick={() => setZoomMode("fit")}
            >
              页面
            </button>
            <button
              type="button"
              className={zoomMode === "width" ? "is-active" : ""}
              onClick={() => setZoomMode("width")}
            >
              宽度
            </button>
            <button
              type="button"
              className={zoomMode === "actual" && zoom === 1 ? "is-active" : ""}
              onClick={() => {
                setZoomMode("actual");
                setZoom(1);
              }}
            >
              100%
            </button>
            <button
              type="button"
              className={spread ? "is-active" : ""}
              onClick={toggleSpread}
              title="双页模式 (D)"
            >
              双页
            </button>
            <button
              type="button"
              className={playback ? "is-active" : ""}
              onClick={togglePlayback}
              title="播放阅读模式 (P)：动图播完自动翻页，翻完换下一本；空格暂停/继续"
            >
              {playback && playing ? (
                <PauseIcon width={11} height={11} />
              ) : (
                <PlayIcon width={11} height={11} />
              )}
              播放
            </button>
          </div>
        </div>
      </footer>
    </div>
  );
}
