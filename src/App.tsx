import { useCallback, useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api } from "./api";
import type { LibraryState, WallpaperLockConfig } from "./types";
import Welcome from "./components/Welcome";
import WorkspaceTabs from "./components/WorkspaceTabs";
import WallpaperLock from "./components/WallpaperLock";
import "./App.css";

type BootPhase = "booting" | "welcome" | "library";

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
      const state = await api.getLibraryState();
      setLastLibraryPath(state.lastLibraryPath);
      setError(state.error);
      if (state.libraryPath) {
        setLibrary(state);
        setPhase("library");
      } else {
        setPhase("welcome");
      }
    } catch (bootError) {
      setError(`启动失败：${String(bootError)}`);
      setPhase("welcome");
    }
  }, []);

  useEffect(() => {
    let cancelled = false;

    void (async () => {
      // 先看壁纸锁：开着就停在锁屏，等输密码；不开才正常加载库。
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
      if (!cancelled) await bootLibrary();
    })();

    return () => {
      cancelled = true;
    };
  }, [bootLibrary]);

  const unlock = useCallback(() => {
    setLocked(false);
    setLockConfig(null);
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
    return (
      <div className="boot-screen">
        <span className="spinner spinner-large" />
        <p>正在打开画匣…</p>
      </div>
    );
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
