import { useEffect, useRef, useState } from "react";
import type { WallpaperSource } from "../types";
import { wallpaperDisplayUrls } from "../wallpapers";

const STAGES = [
  "正在打开画匣…",
  "正在连接画廊库（外接硬盘若在休眠会多等几秒）…",
  "正在读取画集索引…",
  "马上就好…",
];

/**
 * 启动加载屏（管理员 2026-11-27：桌面快捷方式进来要做厚实一点、
 * 富有动效的高质量加载条；壁纸锁的壁纸也塞进来，做好加载过渡）。
 *
 * - 背景：复用壁纸锁的壁纸（没配壁纸就用纯色渐变），慢速 Ken Burns + 多张
 *   交叉淡化，和锁屏无缝衔接——解锁后看到的还是同一张壁纸，只是压上了加载条；
 * - 加载条：粗体三段式（底轨 / 呼吸段 / 扫光），外接硬盘慢加载时有持续反馈；
 * - 文案：分阶段提示，特别说明「外接硬盘休眠中」的等待是正常的。
 */
export default function BootScreen({ walls }: { walls: WallpaperSource[] }) {
  const [urls, setUrls] = useState<string[]>([]);
  useEffect(() => {
    let alive = true;
    const load = () =>
      wallpaperDisplayUrls(walls).then((list) => {
        if (!alive) return;
        setUrls(list.filter((url): url is string => Boolean(url)));
      });
    void load();
    // 首次启动时本地缓存是后端在后台复制的（外接硬盘上的原图动不动十几兆），
    // 一次没取到会退回原地址——硬盘睡着时就是一屏纯色。1.6 秒后再要一次，
    // 缓存刚生成好就能在同一屏里贴上来（管理员 2026-11-27 反馈）。
    const retry = window.setTimeout(() => void load(), 1600);
    return () => {
      alive = false;
      window.clearTimeout(retry);
    };
  }, [walls]);
  const [index, setIndex] = useState(0);
  const [stage, setStage] = useState(0);
  const startRef = useRef(Date.now());
  const [, forceTick] = useState(0);

  // 多张壁纸轮换（8 秒一张），交叉淡化交给 CSS。
  useEffect(() => {
    if (urls.length <= 1) return;
    const timer = window.setInterval(
      () => setIndex((current) => (current + 1) % urls.length),
      8000,
    );
    return () => window.clearInterval(timer);
  }, [urls.length]);

  // 阶段文案 + 秒表：每秒推进一次。
  useEffect(() => {
    const timer = window.setInterval(() => {
      const elapsed = (Date.now() - startRef.current) / 1000;
      setStage(elapsed < 2.5 ? 0 : elapsed < 6 ? 1 : elapsed < 12 ? 2 : 3);
      forceTick((n) => n + 1);
    }, 1000);
    return () => window.clearInterval(timer);
  }, []);

  const elapsed = Math.floor((Date.now() - startRef.current) / 1000);
  const current = urls.length > 0 ? urls[index % urls.length] : null;
  const under = urls.length > 1 ? urls[(index - 1 + urls.length) % urls.length] : null;

  return (
    <div className="boot-screen boot-screen-rich">
      {under && under !== current && (
        <div className="boot-wall is-under" key={`under-${under}`}>
          <img src={under} alt="" draggable={false} aria-hidden="true" />
        </div>
      )}
      {current && (
        <div className="boot-wall" key={current}>
          <img src={current} alt="" draggable={false} aria-hidden="true" />
        </div>
      )}
      <div className="boot-shade" aria-hidden="true" />
      <div className="boot-core">
        <span className="boot-mark" aria-hidden="true" />
        <p className="boot-brand">画匣</p>
        <div className="boot-bar" role="progressbar" aria-label="启动进度">
          <span className="boot-bar-breath" />
          <span className="boot-bar-sweep" />
          <span className="boot-bar-gloss" />
        </div>
        <p className="boot-stage">{STAGES[stage]}</p>
        {stage >= 1 && (
          <p className="boot-elapsed">已等待 {elapsed} 秒 · 硬盘唤醒中不影响使用</p>
        )}
      </div>
    </div>
  );
}
