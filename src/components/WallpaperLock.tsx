import { useEffect, useState, type FormEvent } from "react";
import { api } from "../api";
import type { WallpaperLockConfig } from "../types";
import { wallpaperUrl } from "../wallpapers";

/**
 * 启动壁纸锁：整屏壁纸 + 居中密码框。
 *
 * 管理员定位：防君子不防小人，不做防破解。密码错误只抖动，不计数、不锁定。
 * 没设密码时直接回车就进（默认无密码）。
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

  // 多张时按配置的间隔轮换；只有一张或间隔为 0 就定住。
  useEffect(() => {
    if (urls.length <= 1 || config.intervalSecs <= 0) return;
    const timer = window.setInterval(
      () => setIndex((current) => (current + 1) % urls.length),
      config.intervalSecs * 1000,
    );
    return () => window.clearInterval(timer);
  }, [urls.length, config.intervalSecs]);

  const current = urls.length > 0 ? urls[index % urls.length] : null;

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
      style={current ? { backgroundImage: `url("${current}")` } : undefined}
    >
      <div className="wallpaper-lock-shade" aria-hidden="true" />
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
