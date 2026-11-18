import { useCallback, useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api } from "../api";
import type { ReleaseState } from "../types";
import { CloseIcon, RefreshIcon } from "./Icons";

interface ReleasePanelProps {
  onClose: () => void;
  onNotify: (message: string, kind?: "info" | "success" | "error") => void;
}

/**
 * 「更新画匣」面板。
 *
 * 发布必须在本程序退出后才跑（脚本要结束 archive-gallery.exe 并覆盖它），
 * 所以这里做的是「登记源码目录 + 安排一次退出后发布」，而不是当场构建。
 */
export default function ReleasePanel({ onClose, onNotify }: ReleasePanelProps) {
  const [state, setState] = useState<ReleaseState | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [countdown, setCountdown] = useState<number | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      setState(await api.getReleaseState());
    } catch (error) {
      onNotify(`读取更新设置失败：${String(error)}`, "error");
    } finally {
      setLoading(false);
    }
  }, [onNotify]);

  useEffect(() => {
    void load();
  }, [load]);

  async function persist(next: {
    sourceDir: string | null;
    autoOnExit: boolean;
    relaunchAfter: boolean;
  }) {
    setBusy(true);
    try {
      setState(await api.saveReleaseSettings(
        next.sourceDir,
        next.autoOnExit,
        next.relaunchAfter,
      ));
    } catch (error) {
      onNotify(`保存失败：${String(error)}`, "error");
      // 回滚显示到磁盘上的真实状态
      await load();
    } finally {
      setBusy(false);
    }
  }

  async function pickSourceDir() {
    try {
      const selected = await open({
        title: "选择画匣源码目录（含 package.json 与 scripts/release.mjs）",
        directory: true,
        multiple: false,
      });
      if (typeof selected !== "string") return;
      await persist({
        sourceDir: selected,
        autoOnExit: state?.autoOnExit ?? false,
        relaunchAfter: state?.relaunchAfter ?? true,
      });
      onNotify("源码目录已设置", "success");
    } catch (error) {
      onNotify(`选择目录失败：${String(error)}`, "error");
    }
  }

  /** 立即更新：排好脚本 → 倒数 3 秒 → 退出（脚本接手构建并重开）。 */
  async function updateNow() {
    if (!state?.sourceDir) {
      onNotify("请先选择源码目录", "error");
      return;
    }
    setBusy(true);
    try {
      await api.scheduleRelease();
    } catch (error) {
      setBusy(false);
      onNotify(`安排更新失败：${String(error)}`, "error");
      return;
    }
    setBusy(false);
    setCountdown(3);
  }

  // 倒计时结束后真正退出
  useEffect(() => {
    if (countdown === null) return;
    if (countdown > 0) {
      const timer = window.setTimeout(() => setCountdown(countdown - 1), 1000);
      return () => window.clearTimeout(timer);
    }
    void api.quitForRelease().catch(() => {
      onNotify("退出失败，请手动关闭本程序", "error");
    });
  }, [countdown, onNotify]);

  async function cancelScheduled() {
    try {
      await api.cancelRelease();
      setCountdown(null);
      await load();
      onNotify("已取消这次更新", "info");
    } catch (error) {
      onNotify(`取消失败：${String(error)}`, "error");
    }
  }

  const status = state?.status;
  const lastResult = status?.lastResult ?? "none";
  const lastLabel =
    lastResult === "success"
      ? "上次更新成功"
      : lastResult === "failed"
        ? "上次更新失败"
        : lastResult === "running"
          ? "上次更新被中断"
          : "还没有更新过";

  return (
    <div className="modal-backdrop" role="presentation" onClick={onClose}>
      <div
        className="release-panel"
        role="dialog"
        aria-modal="true"
        aria-label="更新画匣"
        onClick={(event) => event.stopPropagation()}
      >
        <header className="release-panel-head">
          <div>
            <p className="eyebrow">UPDATE</p>
            <h2>更新画匣</h2>
          </div>
          <div className="release-panel-head-actions">
            <button
              type="button"
              className="icon-button"
              onClick={() => void load()}
              aria-label="刷新"
              title="刷新"
            >
              <RefreshIcon width={16} height={16} />
            </button>
            <button
              type="button"
              className="icon-button"
              onClick={onClose}
              aria-label="关闭"
            >
              <CloseIcon width={16} height={16} />
            </button>
          </div>
        </header>

        <div className="release-panel-body">
          {loading && <p className="queue-hint">读取中…</p>}

          {!loading && state && (
            <>
              <section className="release-section">
                <div className="release-row">
                  <div className="release-row-text">
                    <strong>源码目录</strong>
                    <p className="queue-hint">
                      发布要在这里跑 <code>npm run release</code>。
                      目录里必须有 <code>package.json</code> 和{" "}
                      <code>scripts/release.mjs</code>。
                    </p>
                    <p className="release-path">
                      {state.sourceDir ?? "（未设置）"}
                    </p>
                  </div>
                  <button
                    type="button"
                    className="btn btn-quiet btn-compact"
                    disabled={busy}
                    onClick={() => void pickSourceDir()}
                  >
                    选择…
                  </button>
                </div>
              </section>

              <section className="release-section">
                <label className="release-toggle">
                  <input
                    type="checkbox"
                    checked={state.autoOnExit}
                    disabled={busy}
                    onChange={(event) =>
                      void persist({
                        sourceDir: state.sourceDir,
                        autoOnExit: event.target.checked,
                        relaunchAfter: state.relaunchAfter,
                      })
                    }
                  />
                  <span>
                    <strong>关闭画匣时自动更新</strong>
                    <span className="queue-hint">
                      关窗后脚本接手：等进程退出 → 构建 → 安装新版 → 按下面设置决定是否重开。
                    </span>
                  </span>
                </label>

                <label className="release-toggle">
                  <input
                    type="checkbox"
                    checked={state.relaunchAfter}
                    disabled={busy || !state.autoOnExit}
                    onChange={(event) =>
                      void persist({
                        sourceDir: state.sourceDir,
                        autoOnExit: state.autoOnExit,
                        relaunchAfter: event.target.checked,
                      })
                    }
                  />
                  <span>
                    <strong>更新完成后自动重新打开画匣</strong>
                    <span className="queue-hint">
                      不勾的话，更新完就停在那儿，你自己再打开。
                    </span>
                  </span>
                </label>
              </section>

              <section className="release-section">
                <button
                  type="button"
                  className="btn btn-primary"
                  disabled={busy || !state.sourceDir || countdown !== null}
                  onClick={() => void updateNow()}
                >
                  {countdown !== null ? `即将退出（${countdown}）` : "立即更新"}
                </button>
                <p className="queue-hint">
                  点「立即更新」会把画匣关掉，请等它自己重开；期间不要手动启动。
                </p>
                {countdown !== null && (
                  <button
                    type="button"
                    className="btn btn-quiet btn-compact"
                    onClick={() => void cancelScheduled()}
                  >
                    取消
                  </button>
                )}
              </section>

              <section className="release-section">
                <div className="release-row">
                  <div className="release-row-text">
                    <strong>上次结果</strong>
                    <p
                      className={
                        lastResult === "success"
                          ? "release-result is-ok"
                          : lastResult === "failed"
                            ? "release-result is-bad"
                            : "release-result"
                      }
                    >
                      {lastLabel}
                      {status?.lastAt
                        ? `（${new Date(status.lastAt).toLocaleString()}）`
                        : ""}
                    </p>
                    {status?.lastMessage && (
                      <p className="queue-hint">{status.lastMessage}</p>
                    )}
                  </div>
                </div>
                {status?.logPath && (
                  <p className="release-path">日志：{status.logPath}</p>
                )}
                {status?.scriptPath && (
                  <p className="release-path">脚本：{status.scriptPath}</p>
                )}
              </section>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
