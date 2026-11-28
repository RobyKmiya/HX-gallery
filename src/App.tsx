import { useCallback, useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api } from "./api";
import type { LibraryState, WallpaperLockConfig } from "./types";
import Welcome from "./components/Welcome";
import WorkspaceTabs from "./components/WorkspaceTabs";
import { detectDetachedWindow } from "./tabSettings";
import WallpaperLock from "./components/WallpaperLock";
import BootScreen from "./components/BootScreen";
import "./App.css";

type BootPhase = "booting" | "welcome" | "library";

/** 进程内起点（模块加载即埋点）：用来算启动屏至少该露多久。 */
const APP_STARTED_AT = performance.now();
/** 启动屏（原生小窗）最短露面时长：低于这个数，动效还没起来就交接了。 */
const SPLASH_MIN_VISIBLE_MS = 700;
/** 启动动效总时长：库开得再快也让加载屏走完这一段，否则就是「黑一下就进库」。 */
const BOOT_MIN_ANIMATION_MS = 950;

/**
 * A7：拆出的独立标签窗（index.html?detach=token）。
 *
 * 它**不是**第二次启动：不能关启动屏（会去 show 主窗）、不能等启动动效、
 * 不能走壁纸锁（用户已经解过）、也不能 fallback 恢复主窗的标签快照。
 * 一旦走成完整启动流程，新窗会把主窗的 browser 标签全复制过来、每个都往
 * 自己身上挂一个原生引擎，直接卡死（2026-11-28 实机反馈）。
 */
const IS_DETACHED_WINDOW = detectDetachedWindow();

export default function App() {
  const [phase, setPhase] = useState<BootPhase>("booting");
  const [library, setLibrary] = useState<LibraryState | null>(null);
  const [lastLibraryPath, setLastLibraryPath] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  /** 壁纸锁：锁着时先停在锁屏，**不加载库**，输对密码才进。 */
  const [locked, setLocked] = useState(false);
  const [lockConfig, setLockConfig] = useState<WallpaperLockConfig | null>(null);

  const bootLibrary = useCallback(async () => {
    setPhase("booting");
    try {
      // 后台开库模式：Rust 侧在独立线程打开库（外接硬盘唤醒可能要十几秒），
      // 第一次请求多半返回 loading=true——停在加载屏轮询，开好自动进库。
      const deadline = Date.now() + 120_000;
      for (;;) {
        const state = await api.getLibraryState();
        if (state.libraryPath) {
          // 拆窗是新窗，没有启动屏也没有动效，库已由主窗开好：立即进库。
          const remaining = IS_DETACHED_WINDOW
            ? 0
            : BOOT_MIN_ANIMATION_MS - (performance.now() - APP_STARTED_AT);
          if (remaining > 0) {
            await new Promise((resolve) => setTimeout(resolve, remaining));
          }
          setLastLibraryPath(state.lastLibraryPath);
          setError(state.error);
          setLibrary(state);
          setPhase("library");
          return;
        }
        if (!state.loading || Date.now() > deadline) {
          // 开库结束但没有库（首次使用 / 上次的库失效）：进欢迎页带错误提示。
          setLastLibraryPath(state.lastLibraryPath);
          setError(
            state.error ??
              (state.loading ? "库打开超时：外接硬盘可能没就绪，重启应用再试" : null),
          );
          setPhase("welcome");
          return;
        }
        await new Promise((resolve) => setTimeout(resolve, 400));
      }
    } catch (bootError) {
      const message = String(bootError);
      // 双保险：Tauri 窗口先于 setup 闭包创建，首屏命令有极小概率撞上
      // Rust 侧 manage 之前（报 "state not managed"）。退避重试而不是把
      // 欢迎页钉死在报错上（管理员三轮反馈的启动报错）。
      if (/state not managed/.test(message)) {
        for (let attempt = 0; attempt < 20; attempt++) {
          await new Promise((resolve) => setTimeout(resolve, 250));
          try {
            const state = await api.getLibraryState();
            if (state.libraryPath) {
              setLastLibraryPath(state.lastLibraryPath);
              setError(state.error);
              setLibrary(state);
              setPhase("library");
              return;
            }
            if (!state.loading) break;
          } catch (retryError) {
            if (!/state not managed/.test(String(retryError))) break;
          }
        }
      }
      setError(`启动失败：${message}`);
      setPhase("welcome");
    }
  }, []);

  useEffect(() => {
    // 浏览器引擎（嵌入网页标签）要求开新标签：转发给 WorkspaceTabs 已有的
    // open-url-tab 桥，开成一个真正独立的工作台网页标签（有自己的历史栈）。
    let unlisten: (() => void) | null = null;
    let dead = false;
    void import("@tauri-apps/api/event").then(({ listen }) =>
      listen<string>("huaxia://open-url-tab", (event) => {
        const url = event.payload;
        // about:blank / huaxia:newtab → 外壳开自己的「新页」标签（Ctrl+T）。
        if (
          typeof url === "string" &&
          (/^https?:\/\//.test(url) || url === "huaxia:newtab" || url === "about:blank")
        ) {
          window.dispatchEvent(new CustomEvent("huaxia:open-url-tab", { detail: url }));
        }
      }),
    ).then((un) => {
      if (dead) un();
      else unlisten = un;
    });
    return () => {
      dead = true;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    // 交接原生启动屏 → 主窗。两个条件都满足才换：
    // ① 双 rAF：保证 BootScreen 已经真的绘制到屏幕上（mount 时只是提交了 DOM，
    //    以前在这里直接关启动屏，主窗亮起的那一帧还是空的 = 管理员说的「黑屏」）；
    // ② 启动屏至少露 SPLASH_MIN_VISIBLE_MS，让加载动效看得见（不再是「黑一下就进库」）。
    let splashTimer = 0;
    let splashDone = false;
    const handoff = () => {
      requestAnimationFrame(() => {
        requestAnimationFrame(() => {
          if (splashDone) return;
          splashDone = true;
          void api.splashFinish();
        });
      });
    };
    if (!IS_DETACHED_WINDOW) {
      // 只有主窗才交接启动屏。新窗调 splash_finish 会把已经藏起来的主窗
      // 又 show 出来，并抢走焦点（2026-11-28 实机反馈的「两个窗互相盖」）。
      const visibleFor = performance.now() - APP_STARTED_AT;
      splashTimer = window.setTimeout(
        handoff,
        Math.max(0, SPLASH_MIN_VISIBLE_MS - visibleFor),
      );
    }
    let cancelled = false;

    void (async () => {
      // 拆出的新窗不重走锁屏：用户已经解锁过，锁屏只在应用启动时出现一次。
      if (!IS_DETACHED_WINDOW) {
        try {
          const config = await api.getWallpaperLock();
          if (cancelled) return;
          if (config.enabled) {
            setLockConfig(config);
            setLocked(true);
            setPhase("welcome");
            return;
          }
        } catch {
          // 读不到锁配置就当没开锁，不影响进库。
        }
      }
      if (!cancelled) await bootLibrary();
    })();

    return () => {
      cancelled = true;
      window.clearTimeout(splashTimer);
    };
  }, [bootLibrary]);

  const unlock = useCallback(() => {
    // 只解锁，不清 lockConfig：启动加载屏继续用这组壁纸做背景，
    // 解锁 → 加载 → 进库的过渡里壁纸不断片（管理员 2026-11-27）。
    setLocked(false);
    void bootLibrary();
  }, [bootLibrary]);

  const chooseLibrary = useCallback(async () => {
    setBusy(true);
    setError(null);

    try {
      const selected = await open({
        title: "选择画廊库目录",
        directory: true,
        multiple: false,
      });

      if (typeof selected !== "string") return;

      const state = await api.openLibrary(selected);
      setLibrary(state);
      setLastLibraryPath(state.libraryPath ?? selected);
      setPhase("library");
    } catch (chooseError) {
      setError(String(chooseError));
    } finally {
      setBusy(false);
    }
  }, []);

  const refreshLibrary = useCallback(async () => {
    try {
      const state = await api.getLibraryState();
      setLibrary(state);
      setLastLibraryPath(state.lastLibraryPath);
      return state;
    } catch (refreshError) {
      setError(String(refreshError));
      throw refreshError;
    }
  }, []);

  const switchLibrary = useCallback(() => {
    setPhase("welcome");
  }, []);

  // 锁屏优先：锁着时什么都不加载、不显示。
  if (locked && lockConfig) {
    return <WallpaperLock config={lockConfig} onUnlock={unlock} />;
  }

  if (phase === "booting") {
    // 启动加载屏：壁纸锁的壁纸当背景（没配壁纸就是纯色），粗加载条 + 阶段文案；
    // 外接硬盘休眠唤醒慢时给持续反馈（以前一根小转圈看着像卡死）。
    return <BootScreen walls={lockConfig?.walls ?? []} />;
  }

  if (phase === "library" && library?.libraryPath) {
    return (
      <WorkspaceTabs
        library={library}
        onLibraryChanged={refreshLibrary}
        onSwitchLibrary={switchLibrary}
      />
    );
  }

  return (
    <Welcome
      lastLibraryPath={lastLibraryPath}
      error={error}
      busy={busy}
      onChoose={() => void chooseLibrary()}
    />
  );
}
