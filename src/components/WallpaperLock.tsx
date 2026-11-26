import { useEffect, useRef, useState, type FormEvent } from "react";
import { api } from "../api";
import type { WallpaperLockConfig } from "../types";
import { wallpaperUrl } from "../wallpapers";

/**
 * 启动壁纸锁：整屏壁纸 + 居中密码框。
 *
 * 管理员定位：防君子不防小人，不做防破解。密码错误只抖动，不计数、不锁定。
 * 没设密码时直接回车就进（默认无密码）。
 *
 * 2026-11-26 补：壁纸支持**拖拽平移 + 滚轮缩放**（之前是死图看不全）；
 * 加载失败明确提示，不再整屏黑掉装作没生效。
 *
 * 2026-11-27 补（A15）：多张轮换时新壁纸压在旧图上淡入，避免整屏闪一下。
 */
export default function WallpaperLock({
  config,
  onUnlock,
}: {
  config: WallpaperLockConfig;
  onUnlock: () => void;
}) {
  const urls = config.walls
    .map((wall) => wallpaperUrl(wall))
    .filter((url): url is string => Boolean(url));
  const [index, setIndex] = useState(0);
  const [password, setPassword] = useState("");
  const [error, setError] = useState(false);
  const [busy, setBusy] = useState(false);
  const [shakeKey, setShakeKey] = useState(0);
  const [loadFailed, setLoadFailed] = useState(false);
  // 平移 + 缩放：拖画面看细节，滚轮放大缩小，双击复位。
  const [zoom, setZoom] = useState(1);
  const [offset, setOffset] = useState({ x: 0, y: 0 });
  const dragRef = useRef<{ x: number; y: number; ox: number; oy: number } | null>(null);

  const current = urls.length > 0 ? urls[index % urls.length] : null;
  // A15 交叉淡化：换片时保留上一张垫底，新片压上来淡入，淡入结束再撤掉旧片。
  const [outgoing, setOutgoing] = useState<string | null>(null);
  const crossfadeTimer = useRef<number | null>(null);

  // 多张时按配置的间隔轮换；只有一张或间隔为 0 就定住。
  useEffect(() => {
    if (urls.length <= 1 || config.intervalSecs <= 0) return;
    const timer = window.setInterval(
      () => setIndex((current) => (current + 1) % urls.length),
      config.intervalSecs * 1000,
    );
    return () => window.clearInterval(timer);
  }, [urls.length, config.intervalSecs]);

  // 换壁纸时把视角复位，并启动交叉淡化。
  useEffect(() => {
    setZoom(1);
    setOffset({ x: 0, y: 0 });
    setLoadFailed(false);
    if (crossfadeTimer.current !== null) {
      window.clearTimeout(crossfadeTimer.current);
      crossfadeTimer.current = null;
    }
    if (urls.length > 1) {
      const previous = urls[(index - 1 + urls.length) % urls.length];
      if (previous && previous !== urls[index % urls.length]) {
        setOutgoing(previous);
        crossfadeTimer.current = window.setTimeout(() => {
          setOutgoing(null);
          crossfadeTimer.current = null;
        }, 1200);
      }
    }
    return () => {
      if (crossfadeTimer.current !== null) {
        window.clearTimeout(crossfadeTimer.current);
        crossfadeTimer.current = null;
      }
    };
  }, [index, urls.length]);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (busy) return;
    setBusy(true);
    try {
      const ok = await api.verifyWallpaperPassword(password);
      if (ok) {
        onUnlock();
        return;
      }
      setError(true);
      setShakeKey((key) => key + 1);
      setPassword("");
    } catch {
      setError(true);
      setShakeKey((key) => key + 1);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div
      className="wallpaper-lock"
      onWheel={(event) => {
        if (!current) return;
        event.preventDefault();
        const factor = event.deltaY < 0 ? 1.12 : 1 / 1.12;
        setZoom((value) => Math.min(6, Math.max(1, value * factor)));
      }}
      onDoubleClick={() => {
        setZoom(1);
        setOffset({ x: 0, y: 0 });
      }}
      onPointerDown={(event) => {
        if (!current || event.button !== 0) return;
        dragRef.current = { x: event.clientX, y: event.clientY, ox: offset.x, oy: offset.y };
        event.currentTarget.setPointerCapture(event.pointerId);
      }}
      onPointerMove={(event) => {
        const drag = dragRef.current;
        if (!drag) return;
        setOffset({
          x: drag.ox + (event.clientX - drag.x),
          y: drag.oy + (event.clientY - drag.y),
        });
      }}
      onPointerUp={() => {
        dragRef.current = null;
      }}
    >
      {outgoing && !loadFailed && (
        <img
          className="wallpaper-lock-img"
          src={outgoing}
          alt=""
          draggable={false}
          aria-hidden="true"
          style={{
            transform: `translate(${offset.x}px, ${offset.y}px) scale(${zoom})`,
          }}
        />
      )}
      {current && !loadFailed && (
        <img
          key={current}
          className={`wallpaper-lock-img${outgoing ? " is-entering" : ""}`}
          src={current}
          alt=""
          draggable={false}
          onError={() => setLoadFailed(true)}
          style={{
            transform: `translate(${offset.x}px, ${offset.y}px) scale(${zoom})`,
            cursor: zoom > 1 ? "grab" : "default",
          }}
        />
      )}
      <div className="wallpaper-lock-shade" aria-hidden="true" />
      {(!current || loadFailed) && (
        <p className="wallpaper-lock-hint">
          {urls.length === 0
            ? "还没有壁纸：去 设置 → 壁纸锁 添加一张（不挡进入）"
            : "这张壁纸加载失败（文件可能被移动或删除），去 设置 → 壁纸锁 重新添加"}
        </p>
      )}
      {current && !loadFailed && (
        <p className="wallpaper-lock-tip">滚轮缩放 · 拖拽平移 · 双击复位</p>
      )}
      <div className={`wallpaper-lock-body${error ? " is-error" : ""}`} key={shakeKey}>
        <form className={`wallpaper-lock-card${error ? " is-error" : ""}`} onSubmit={submit}>
          <span className="wallpaper-lock-mark" aria-hidden="true" />
          <p className="wallpaper-lock-title">画匣</p>
          <input
            type="password"
            autoFocus
            value={password}
            onChange={(event) => {
              setPassword(event.target.value);
              if (error) setError(false);
            }}
            placeholder={error ? "密码不对，再试一次" : "请输入密码（没设密码直接回车）"}
            className="wallpaper-lock-input"
          />
          <button type="submit" className="btn btn-primary" disabled={busy}>
            {busy ? "验证中…" : "进入画匣"}
          </button>
        </form>
      </div>
    </div>
  );
}
