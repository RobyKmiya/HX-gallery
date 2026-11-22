import { memo, useEffect, useState } from "react";
import { assetUrl } from "../api";
import type { SeriesSummary } from "../types";
import { BookIcon } from "./Icons";

interface SeriesCardProps {
  series: SeriesSummary;
  onOpen: (series: SeriesSummary) => void;
}

/**
 * 合集卡：主网格里代表一整条连载。
 *
 * 封面按成员轮换（默认 2.6 秒一换，鼠标悬停暂停）——所有封面都预挂载、
 * 用透明度交叉淡入，切的时候不会闪白，也不会重复请求图片。
 */
function SeriesCard({ series, onOpen }: SeriesCardProps) {
  const items = series.items;
  const withCover = items.filter((item) => item.coverPath);
  // 指定的封面排最前，其余按集数顺序跟后面轮换。
  const ordered = series.coverArchiveId
    ? [
        ...withCover.filter((item) => item.archiveId === series.coverArchiveId),
        ...withCover.filter((item) => item.archiveId !== series.coverArchiveId),
      ]
    : withCover;
  const covers = ordered.map((item) => assetUrl(item.coverPath as string));

  const [index, setIndex] = useState(0);
  const [paused, setPaused] = useState(false);

  useEffect(() => {
    setIndex(0);
  }, [series.id]);

  useEffect(() => {
    if (covers.length < 2 || paused) return;
    const timer = window.setInterval(() => {
      setIndex((current) => (current + 1) % covers.length);
    }, 2600);
    return () => window.clearInterval(timer);
  }, [covers.length, paused]);

  const totalPages = items.reduce((sum, item) => sum + item.pageCount, 0);

  return (
    <article
      className="archive-card is-series"
      role="button"
      tabIndex={0}
      aria-label={`打开合集 ${series.name}`}
      onClick={() => onOpen(series)}
      onPointerEnter={() => setPaused(true)}
      onPointerLeave={() => setPaused(false)}
      onKeyDown={(event) => {
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          onOpen(series);
        }
      }}
    >
      <div className="archive-cover series-cover">
        {covers.length > 0 ? (
          covers.map((src, position) => (
            <img
              key={src}
              src={src}
              alt=""
              loading="lazy"
              decoding="async"
              draggable={false}
              className={position === index % covers.length ? "is-active" : ""}
            />
          ))
        ) : (
          <div className="cover-placeholder">
            <BookIcon width={26} height={26} />
            <span>{series.name.slice(0, 1)}</span>
          </div>
        )}

        <span className="format-badge is-series">合集</span>
        <span className="series-count-badge">
          <BookIcon width={11} height={11} />
          {series.itemCount} 话
        </span>

        {covers.length > 1 && (
          <span className="series-dots" aria-hidden="true">
            {covers.map((src, position) => (
              <i
                key={src}
                className={position === index % covers.length ? "is-on" : ""}
              />
            ))}
          </span>
        )}
      </div>

      <div className="archive-body">
        <h3 title={series.note ?? series.name}>{series.name}</h3>
        <div className="archive-meta">
          <span>{series.itemCount} 话</span>
          <span className="meta-dot" />
          <span>{totalPages} 页</span>
          <span className="meta-dot" />
          <span>合集</span>
        </div>
        {series.note && <p className="archive-note">{series.note}</p>}
      </div>
    </article>
  );
}

SeriesCard.displayName = "SeriesCard";

export default memo(SeriesCard);
