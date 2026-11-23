import { memo, useEffect, useState } from "react";
import { assetUrl } from "../api";
import { formatBytes, formatDate, formatDateTime } from "../format";
import type { ArchiveCardData } from "../types";
import {
  AlertIcon,
  BookIcon,
  CheckIcon,
  ChevronDownIcon,
  ChevronUpIcon,
  HeartIcon,
  LogoIcon,
  StarIcon,
} from "./Icons";

interface ArchiveCardProps {
  card: ArchiveCardData;
  onOpen: (card: ArchiveCardData) => void;
  selectionMode?: boolean;
  selected?: boolean;
  onToggleSelect?: (id: number) => void;
  onQuickEdit?: (card: ArchiveCardData) => void;
  onRead?: (card: ArchiveCardData) => void;
  eager?: boolean;
  /** 队列视图：把这本在待看队列里往前/往后挪一格（不传＝不显示按钮）。 */
  onQueueMove?: (direction: -1 | 1) => void;
  /** 右键菜单：传了才启用（宿主负责定位与菜单内容）。 */
  onContextMenu?: (card: ArchiveCardData, x: number, y: number) => void;
  /** 队列两端禁用对应方向的按钮。 */
  queueEdges?: { upDisabled: boolean; downDisabled: boolean };
  /** 双击卡片：直接进阅读（模式由设置决定）。 */
  onOpenReader?: (card: ArchiveCardData) => void;
}

/**
 * 单张画集卡片。
 *
 * 用 `memo` 包住：滚动模式一次能挂上千张卡，而父组件的状态（搜索框输入、
 * 选中集合、Toast、浮层开关…）每次变化都会重渲染整棵树。这里的 props 全是
 * 稳定引用（卡片对象来自同一份 `items`，回调是 `useState` 的 setter 或
 * `useCallback`），所以浅比较几乎总能短路，只重渲染真正变化的那几张。
 */
function ArchiveCard({
  card,
  onOpen,
  onOpenReader,
  selectionMode = false,
  selected = false,
  onToggleSelect,
  onQuickEdit,
  onRead,
  eager = false,
  onQueueMove,
  queueEdges,
  onContextMenu,
}: ArchiveCardProps) {
  const [imageFailed, setImageFailed] = useState(false);

  useEffect(() => {
    setImageFailed(false);
  }, [card.coverPath]);

  const cover =
    card.coverPath && !imageFailed ? assetUrl(card.coverPath) : null;

  const hasProgress = card.lastPage > 0 && card.pageCount > 0;
  const finished = hasProgress && card.lastPage >= card.pageCount;
  const progressPercent = hasProgress
    ? Math.min(100, Math.round((card.lastPage / card.pageCount) * 100))
    : 0;
  const readLabel = finished
    ? "重新阅读"
    : hasProgress
      ? "继续阅读"
      : "开始阅读";

  const activate = () => {
    if (selectionMode && onToggleSelect) {
      onToggleSelect(card.id);
    } else {
      onOpen(card);
    }
  };

  return (
    <article
      data-card-id={card.id}
      className={`archive-card${card.missing ? " is-missing" : ""}${
        selectionMode ? " is-selecting" : ""
      }${selected ? " is-selected" : ""}`}
      role="button"
      tabIndex={0}
      aria-label={
        selectionMode
          ? `${selected ? "取消选择" : "选择"} ${card.title}`
          : `打开 ${card.title}`
      }
      aria-pressed={selectionMode ? selected : undefined}
      onClick={activate}
      onDoubleClick={(event) => {
        if (!onOpenReader) return;
        // 双击是「直接开看」：别让单击的详情再拦一次。
        event.preventDefault();
        event.stopPropagation();
        onOpenReader(card);
      }}
      onContextMenu={
        onContextMenu
          ? (event) => {
              event.preventDefault();
              onContextMenu(card, event.clientX, event.clientY);
            }
          : undefined
      }
      onKeyDown={(event) => {
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          activate();
        }
      }}
    >
      <div className="archive-cover">
        {cover ? (
          <img
            src={cover}
            alt=""
            loading={eager ? "eager" : "lazy"}
            decoding={eager ? "sync" : "async"}
            draggable={false}
            onError={() => setImageFailed(true)}
          />
        ) : (
          <div className="cover-placeholder">
            <LogoIcon width={28} height={28} />
            <span>{card.title.slice(0, 1).toUpperCase()}</span>
          </div>
        )}

        <span className="format-badge">{card.ext.toUpperCase()}</span>

        <span className="rating-badge" title="评分">
          <StarIcon width={11} height={11} />
          {card.rating}
        </span>

        {hasProgress && (
          <span
            className={`progress-badge${finished ? " is-finished" : ""}`}
            title={`上次读到第 ${card.lastPage} 页${
              card.lastReadAt ? ` · ${formatDateTime(card.lastReadAt)}` : ""
            }`}
          >
            <BookIcon width={10} height={10} />
            {finished ? "已读完" : `${card.lastPage} / ${card.pageCount}`}
          </span>
        )}

        {hasProgress && (
          <span className="cover-progress-track" aria-hidden="true">
            <span style={{ width: `${progressPercent}%` }} />
          </span>
        )}

        {selectionMode && (
          <span className={`select-check${selected ? " is-on" : ""}`}>
            {selected && <CheckIcon width={12} height={12} />}
          </span>
        )}

        {card.missing && (
          <div className="missing-overlay">
            <AlertIcon width={20} height={20} />
            文件缺失
          </div>
        )}

        {!selectionMode && onQueueMove && (
          <span className="archive-queue-controls">
            <button
              type="button"
              className="archive-quick is-queue"
              title="在队列里前移一格"
              aria-label={`把 ${card.title} 在队列里前移`}
              disabled={queueEdges?.upDisabled}
              onClick={(event) => {
                event.stopPropagation();
                onQueueMove(-1);
              }}
              onKeyDown={(event) => event.stopPropagation()}
            >
              <ChevronUpIcon width={13} height={13} />
            </button>
            <button
              type="button"
              className="archive-quick is-queue"
              title="在队列里后移一格"
              aria-label={`把 ${card.title} 在队列里后移`}
              disabled={queueEdges?.downDisabled}
              onClick={(event) => {
                event.stopPropagation();
                onQueueMove(1);
              }}
              onKeyDown={(event) => event.stopPropagation()}
            >
              <ChevronDownIcon width={13} height={13} />
            </button>
          </span>
        )}

        {!selectionMode && <div className="archive-hover-hint">查看详情</div>}

        {!selectionMode && onRead && (
          <button
            type="button"
            className="archive-quick is-read"
            title={readLabel}
            aria-label={`${readLabel} ${card.title}`}
            onClick={(event) => {
              event.stopPropagation();
              onRead(card);
            }}
            onKeyDown={(event) => event.stopPropagation()}
          >
            <BookIcon width={14} height={14} />
          </button>
        )}

        {!selectionMode && onQuickEdit && (
          <button
            type="button"
            className="archive-quick"
            title="收藏到…"
            aria-label={`把 ${card.title} 收藏到收藏夹`}
            onClick={(event) => {
              event.stopPropagation();
              onQuickEdit(card);
            }}
            onKeyDown={(event) => event.stopPropagation()}
          >
            <HeartIcon width={14} height={14} />
          </button>
        )}
      </div>

      <div className="archive-body">
        <h3 title={card.fileName}>{card.title}</h3>
        <div className="archive-meta">
          <span>{card.pageCount > 0 ? `${card.pageCount} 页` : "页数未知"}</span>
          <span className="meta-dot" />
          <span>{formatBytes(card.fileSize)}</span>
          <span className="meta-dot" />
          <span>{formatDate(card.addedAt)}</span>
        </div>
        {card.note && <p className="archive-note">{card.note}</p>}
        {card.error && (
          <div className="archive-warning" title={card.error}>
            <AlertIcon width={12} height={12} />
            索引异常
          </div>
        )}
      </div>
    </article>
  );
}

ArchiveCard.displayName = "ArchiveCard";

export default memo(ArchiveCard);
