import { useEffect, useRef, useState, type FormEvent } from "react";
import { api } from "../api";
import type { WallpaperLockConfig } from "../types";
import { wallpaperUrl } from "../wallpapers";

/**
 * 启动壁纸锁：整屏壁纸 + 居中密码框。
 *
 * 管理员定位：防君子不防小人，不做防破解。密码错误只抖动，不计数、不锁定。
 * 没设密码时「进入画匣」直接就进（后端空密码恒过）。
 *
 * 2026-11-27 改（管理员反馈）：
 * - 壁纸**不再支持拖动平移/滚轮缩放**——不好；显示区域改在
 *   设置 → 壁纸锁 里拖这张图来定（crop，object-position 百分比）；
 * - 无密码时界面就是一颗大按钮，按下立即给「正在进入…」反馈
 *   （以前按了没反应，其实是外接硬盘加载慢，看起来像坏了）；
 * - 2026-11-26 补：加载失败明确提示；A15 多张轮换交叉淡化。
 */
export default function WallpaperLock({
  config,
  onUnlock,
}: {
  config: WallpaperLockConfig;
  onUnlock: () => void;
}) {
  const slides = config.walls
    .map((wall) => ({ url: wallpaperUrl(wall), crop: wall.crop ?? null }))
    .filter((slide): slide is { url: string; crop: { x: number; y: number } | null } =>
      Boolean(slide.url),
    );
  const [index, setIndex] = useState(0);
  const [password, setPassword] = useState("");
  const [error, setError] = useState(false);
  const [busy, setBusy] = useState(false);
  const [shakeKey, setShakeKey] = useState(0);
  const [loadFailed, setLoadFailed] = useState(false);
  const hasPassword = config.hasPassword === true;

  const current = slides.length > 0 ? slides[index % slides.length] : null;
  const crop = current?.crop ?? null;
  const position = crop ? `${crop.x}% ${crop.y}%` : "50% 50%";
  // A15 交叉淡化：换片时保留上一张垫底，新片压上来淡入，淡入结束再撤掉旧片。
  const [outgoing, setOutgoing] = useState<string | null>(null);
  const crossfadeTimer = useRef<number | null>(null);

  // 多张时按配置的间隔轮换；只有一张或间隔为 0 就定住。
  useEffect(() => {
    if (slides.length <= 1 || config.intervalSecs <= 0) return;
    const timer = window.setInterval(
      () => setIndex((current) => (current + 1) % slides.length),
      config.intervalSecs * 1000,
    );
    return () => window.clearInterval(timer);
  }, [slides.length, config.intervalSecs]);

  // 换壁纸时启动交叉淡化。
  useEffect(() => {
    setLoadFailed(false);
    if (crossfadeTimer.current !== null) {
      window.clearTimeout(crossfadeTimer.current);
      crossfadeTimer.current = null;
    }
    if (slides.length > 1) {
      const previous = slides[(index - 1 + slides.length) % slides.length];
      if (previous && previous.url !== slides[index % slides.length].url) {
        setOutgoing(previous.url);
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
  }, [index, slides.length]);

  const enter = async () => {
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

  const submit = (event: FormEvent) => {
    event.preventDefault();
    void enter();
  };

  return (
    <div className="wallpaper-lock">
      {outgoing && !loadFailed && (
        <img
          className="wallpaper-lock-img"
          src={outgoing}
          alt=""
          draggable={false}
          aria-hidden="true"
          style={{ objectPosition: position }}
        />
      )}
      {current && !loadFailed && (
        <img
          key={current.url}
          className={`wallpaper-lock-img${outgoing ? " is-entering" : ""}`}
          src={current.url}
          alt=""
          draggable={false}
          onError={() => setLoadFailed(true)}
          style={{ objectPosition: position }}
        />
      )}
      <div className="wallpaper-lock-shade" aria-hidden="true" />
      {!current && (
        <p className="wallpaper-lock-hint">
          还没有壁纸：去 设置 → 壁纸锁 添加一张（不挡进入）
        </p>
      )}
      {current && loadFailed && (
        <p className="wallpaper-lock-hint">
          这张壁纸加载失败（文件可能被移动或删除），去 设置 → 壁纸锁 重新添加
        </p>
      )}
      <div className={`wallpaper-lock-body${error ? " is-error" : ""}`} key={shakeKey}>
        <form className={`wallpaper-lock-card${error ? " is-error" : ""}`} onSubmit={submit}>
          <span className="wallpaper-lock-mark" aria-hidden="true" />
          <p className="wallpaper-lock-title">画匣</p>
          {hasPassword ? (
            <>
              <input
                type="password"
                autoFocus
                value={password}
                onChange={(event) => {
                  setPassword(event.target.value);
                  if (error) setError(false);
                }}
                placeholder={error ? "密码不对，再试一次" : "请输入密码"}
                className="wallpaper-lock-input"
              />
              <button type="submit" className="btn btn-primary" disabled={busy}>
                {busy ? "验证中…" : "进入画匣"}
              </button>
            </>
          ) : (
            <>
              <p className="wallpaper-lock-nopass">未设密码，点一下就进</p>
              <button
                type="submit"
                className="btn btn-primary wallpaper-lock-enter"
                disabled={busy}
              >
                {busy ? "正在进入…" : "进入画匣"}
              </button>
            </>
          )}
        </form>
      </div>
    </div>
  );
}
