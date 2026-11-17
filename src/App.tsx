import { useCallback, useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api } from "./api";
import type { LibraryState } from "./types";
import LibraryView from "./components/LibraryView";
import Welcome from "./components/Welcome";
import "./App.css";

type BootPhase = "booting" | "welcome" | "library";

export default function App() {
  const [phase, setPhase] = useState<BootPhase>("booting");
  const [library, setLibrary] = useState<LibraryState | null>(null);
  const [lastLibraryPath, setLastLibraryPath] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let cancelled = false;

    void (async () => {
      try {
        const state = await api.getLibraryState();
        if (cancelled) return;

        setLastLibraryPath(state.lastLibraryPath);
        setError(state.error);

        if (state.libraryPath) {
          setLibrary(state);
          setPhase("library");
        } else {
          setPhase("welcome");
        }
      } catch (bootError) {
        if (cancelled) return;
        setError(`启动失败：${String(bootError)}`);
        setPhase("welcome");
      }
    })();

    return () => {
      cancelled = true;
    };
  }, []);

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
      <LibraryView
        key={library.libraryPath}
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
