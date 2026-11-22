import { useEffect, useState } from "react";
import { api, assetUrl } from "../api";
import type { SeriesItem, SeriesSummary } from "../types";
import {
  AlertIcon,
  BookIcon,
  ChevronDownIcon,
  ChevronUpIcon,
  CloseIcon,
  StarIcon,
  TrashIcon,
} from "./Icons";

interface SeriesDrawerProps {
  series: SeriesSummary;
  onClose: () => void;
  /** 有改动后让宿主重新拉数据（网格与合集卡都要跟着变）。 */
  onChanged: () => void;
  /** 直接读某一话。 */
  onReadArchive: (archiveId: number) => void;
  onNotify: (message: string, kind?: "info" | "success" | "error") => void;
}

/**
 * 合集详情：改名字、看全部封面、调顺序、改集数、指定封面、移出成员、拆散。
 *
 * 顺序与集数的修改都立刻落库（没有「保存」按钮）——这类微调边改边看才顺手，
 * 攒着等保存反而容易忘。
 */
export default function SeriesDrawer({
  series,
  onClose,
  onChanged,
  onReadArchive,
  onNotify,
}: SeriesDrawerProps) {
  const [items, setItems] = useState<SeriesItem[]>(series.items);
  const [name, setName] = useState(series.name);
  const [note, setNote] = useState(series.note ?? "");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    setItems(series.items);
    setName(series.name);
    setNote(series.note ?? "");
  }, [series]);

  useEffect(() => {
    const handleKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", handleKey);
    return () => window.removeEventListener("keydown", handleKey);
  }, [onClose]);

  const move = async (index: number, delta: -1 | 1) => {
    const target = index + delta;
    if (target < 0 || target >= items.length) return;
    const next = [...items];
    [next[index], next[target]] = [next[target], next[index]];
    setItems(next);
    try {
      await api.seriesReorder(series.id, next.map((item) => item.archiveId));
      onChanged();
    } catch (error) {
      onNotify(`调序失败：${String(error)}`, "error");
      setItems(series.items);
    }
  };

  const commitLabel = async (archiveId: number, label: string) => {
    try {
      await api.seriesSetLabel(series.id, archiveId, label.trim() || null);
      onChanged();
    } catch (error) {
      onNotify(`改集数失败：${String(error)}`, "error");
    }
  };

  const setCover = async (archiveId: number) => {
    try {
      await api.seriesSetCover(series.id, archiveId);
      onNotify("已设为合集封面", "success");
      onChanged();
    } catch (error) {
      onNotify(`设置封面失败：${String(error)}`, "error");
    }
  };

  const removeItem = async (archiveId: number) => {
    setBusy(true);
    try {
      await api.seriesRemoveArchive(series.id, archiveId);
      onNotify("已移出合集（少于 2 话会自动拆散）", "success");
      onChanged();
    } catch (error) {
      onNotify(`移出失败：${String(error)}`, "error");
    } finally {
      setBusy(false);
    }
  };

  const saveName = async () => {
    const trimmed = name.trim();
    if (!trimmed || trimmed === series.name) return;
    try {
      await api.updateSeries(series.id, trimmed, note.trim() || null);
      onNotify("合集已更新", "success");
      onChanged();
    } catch (error) {
      onNotify(`保存失败：${String(error)}`, "error");
    }
  };

  const dissolve = async () => {
    setBusy(true);
    try {
      await api.dissolveSeries(series.id);
      onNotify("合集已拆散，各话回到画廊", "success");
      onChanged();
      onClose();
    } catch (error) {
      onNotify(`拆散失败：${String(error)}`, "error");
    } finally {
      setBusy(false);
    }
  };

  return (
    <aside className="series-drawer" aria-label={`合集 ${series.name}`}>
      <header className="series-drawer-head">
        <div>
          <p className="eyebrow">SERIES</p>
          <input
            className="series-name-input"
            value={name}
            onChange={(event) => setName(event.target.value)}
            onBlur={() => void saveName()}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                event.currentTarget.blur();
              }
            }}
            aria-label="合集名字"
          />
        </div>
        <button
          type="button"
          className="icon-button"
          onClick={onClose}
          aria-label="关闭合集详情"
          title="关闭 (Esc)"
        >
          <CloseIcon width={14} height={14} />
        </button>
      </header>

      <input
        className="series-note-input"
        value={note}
        placeholder="简介（可留空）"
        onChange={(event) => setNote(event.target.value)}
        onBlur={() => void saveName()}
      />

      <div className="series-items">
        {items.map((item, index) => (
          <div
            className={`series-item${item.archiveId === series.coverArchiveId ? " is-cover" : ""}`}
            key={item.archiveId}
          >
            <span className="series-item-index">{index + 1}</span>

            <button
              type="button"
              className="series-item-thumb"
              onClick={() => onReadArchive(item.archiveId)}
              title="阅读这一话"
              aria-label={`阅读 ${item.title}`}
            >
              {item.coverPath ? (
                <img src={assetUrl(item.coverPath)} alt="" loading="lazy" />
              ) : (
                <BookIcon width={16} height={16} />
              )}
            </button>

            <div className="series-item-main">
              <input
                className="series-label-input"
                defaultValue={item.label ?? ""}
                placeholder={`第 ${index + 1} 话`}
                onBlur={(event) => void commitLabel(item.archiveId, event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Enter") {
                    event.preventDefault();
                    event.currentTarget.blur();
                  }
                }}
                aria-label="这一话的名字"
              />
              <p className="series-item-title" title={item.title}>
                {item.title}
              </p>
              <span className="series-item-meta">
                {item.pageCount} 页
                {item.missing && (
                  <em className="series-missing">
                    <AlertIcon width={10} height={10} /> 文件缺失
                  </em>
                )}
              </span>
            </div>

            <div className="series-item-actions">
              <button
                type="button"
                className="icon-button"
                onClick={() => void move(index, -1)}
                disabled={index === 0}
                title="上移"
                aria-label="上移"
              >
                <ChevronUpIcon width={13} height={13} />
              </button>
              <button
                type="button"
                className="icon-button"
                onClick={() => void move(index, 1)}
                disabled={index === items.length - 1}
                title="下移"
                aria-label="下移"
              >
                <ChevronDownIcon width={13} height={13} />
              </button>
              <button
                type="button"
                className={`icon-button${item.archiveId === series.coverArchiveId ? " is-active" : ""}`}
                onClick={() => void setCover(item.archiveId)}
                title="设为合集封面"
                aria-label="设为合集封面"
              >
                <StarIcon width={13} height={13} />
              </button>
              <button
                type="button"
                className="icon-button"
                onClick={() => void removeItem(item.archiveId)}
                disabled={busy}
                title="移出合集（回到画廊）"
                aria-label="移出合集"
              >
                <TrashIcon width={13} height={13} />
              </button>
            </div>
          </div>
        ))}
      </div>

      <footer className="series-drawer-foot">
        <span>{items.length} 话 · 封面轮换</span>
        <button
          type="button"
          className="btn btn-ghost btn-compact"
          onClick={() => void dissolve()}
          disabled={busy}
        >
          拆散合集
        </button>
      </footer>
    </aside>
  );
}
