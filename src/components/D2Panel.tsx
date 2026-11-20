import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../api";
import type { D2Done, D2FetchReport } from "../types";

interface D2PanelProps {
  onClose: () => void;
  onNotify: (message: string, kind?: "info" | "success" | "error") => void;
}

/** 常用入口。只是快捷方式，不是白名单——地址栏可以填任何站点。 */
const QUICK_SITES: { label: string; url: string; hint?: string }[] = [
  { label: "登录 e-hentai", url: "https://forums.e-hentai.org/index.php?act=Login&CODE=01" },
  { label: "e-hentai", url: "https://e-hentai.org/" },
  { label: "exhentai", url: "https://exhentai.org/" },
  { label: "nhentai", url: "https://nhentai.net/" },
  { label: "18comic", url: "https://18comic.vip/" },
  { label: "wnacg", url: "https://www.wnacg.com/" },
  { label: "pica", url: "https://pica.fanjugou18.top/" },
  { label: "h-comic", url: "https://h-comic.com/" },
];

/**
 * D2 下载测试入口。
 *
 * 两个动作刻意分开：
 * - **打开**：单纯把地址塞进浏览窗口，任意站点都行，不抓取；
 * - **开始抓取**：只在作品详情页用，目前解析器覆盖 exhentai / e-hentai。
 *
 * 分开的原因：exhentai 未登录会 302 到 `?poni=no` 并显示纯白页，
 * 以前的流程一按就进抓取，用户只能看到白屏和超时。现在可以先开浏览器登录。
 */
export default function D2Panel({ onClose, onNotify }: D2PanelProps) {
  const [browseUrl, setBrowseUrl] = useState("https://e-hentai.org/");
  const [url, setUrl] = useState("");
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState("");
  const [error, setError] = useState("");
  const [report, setReport] = useState<D2FetchReport | null>(null);

  useEffect(() => {
    const unlistenStatus = listen<{ message: string }>("d2://status", (event) => {
      setStatus(event.payload.message);
    });
    const unlistenError = listen<string>("d2://error", (event) => {
      setStatus("");
      setError(event.payload);
      onNotify(`浏览窗口打不开：${event.payload}`, "error");
    });
    const unlistenDone = listen<D2Done>("d2://done", (event) => {
      const payload = event.payload;
      setBusy(false);
      setStatus("");
      if (payload.ok && payload.report) {
        setReport(payload.report);
        setError("");
        onNotify(`D2 抓取完成：${payload.report.title}`, "success");
      } else {
        const message = payload.error ?? "抓取失败";
        setError(message);
        onNotify(`D2 抓取失败：${message}`, "error");
      }
    });
    return () => {
      void unlistenStatus.then((unlisten) => unlisten());
      void unlistenError.then((unlisten) => unlisten());
      void unlistenDone.then((unlisten) => unlisten());
    };
  }, [onNotify]);

  async function openBrowser(target?: string) {
    const next = (target ?? browseUrl).trim();
    if (!next) {
      onNotify("请填写网址", "error");
      return;
    }
    setBrowseUrl(next);
    setError("");
    try {
      await api.d2Open(next);
    } catch (err) {
      const message = String(err);
      setError(message);
      onNotify(`打开失败：${message}`, "error");
    }
  }

  async function nav(action: "back" | "forward" | "reload" | "close") {
    try {
      if (action === "back") await api.d2Back();
      else if (action === "forward") await api.d2Forward();
      else if (action === "reload") await api.d2Reload();
      else await api.d2CloseWindow();
    } catch (err) {
      onNotify(String(err), "error");
    }
  }

  async function run() {
    if (!url.trim()) {
      onNotify("请填写作品页网址", "error");
      return;
    }
    setReport(null);
    setError("");
    setStatus("正在打开浏览窗口…");
    setBusy(true);
    try {
      await api.d2Start(url.trim());
    } catch (err) {
      setBusy(false);
      setStatus("");
      const message = String(err);
      setError(message);
      onNotify(`D2 抓取失败：${message}`, "error");
    }
  }

  async function cancel() {
    try {
      await api.d2Cancel();
    } catch {
      // 取消失败不影响界面恢复
    }
    setBusy(false);
    setStatus("");
    onNotify("已取消本次抓取", "info");
  }

  return (
    <div className="modal-backdrop" role="presentation" onClick={onClose}>
      <div
        className="release-panel"
        role="dialog"
        aria-modal="true"
        aria-label="D2 下载测试"
        onClick={(event) => event.stopPropagation()}
      >
        <header className="release-panel-head">
          <div>
            <p className="eyebrow">D2 · M1</p>
            <h2>受控浏览器下载测试</h2>
          </div>
          <button type="button" className="btn btn-quiet btn-compact" onClick={onClose}>
            关闭
          </button>
        </header>

        <div className="release-panel-body">
          <section className="release-section">
            <div className="release-row-text">
              <strong>浏览器</strong>
              <p className="queue-hint">
                地址栏可以填任何站点，没有域名限制。窗口和主程序共用登录态，登一次就够。
              </p>
              <div style={{ display: "flex", gap: 8, marginTop: 8 }}>
                <input
                  value={browseUrl}
                  onChange={(event) => setBrowseUrl(event.target.value)}
                  onKeyDown={(event) => {
                    if (event.key === "Enter") void openBrowser();
                  }}
                  placeholder="https://..."
                  spellCheck={false}
                  style={{
                    flex: 1,
                    padding: "8px 10px",
                    borderRadius: 6,
                    border: "1px solid var(--border-strong)",
                    background: "var(--panel-raised)",
                    color: "var(--text)",
                  }}
                />
                <button type="button" className="btn btn-primary" onClick={() => void openBrowser()}>
                  打开
                </button>
              </div>
              <div style={{ display: "flex", gap: 6, marginTop: 8, flexWrap: "wrap" }}>
                <button type="button" className="btn btn-quiet btn-compact" onClick={() => void nav("back")}>
                  后退
                </button>
                <button type="button" className="btn btn-quiet btn-compact" onClick={() => void nav("forward")}>
                  前进
                </button>
                <button type="button" className="btn btn-quiet btn-compact" onClick={() => void nav("reload")}>
                  刷新
                </button>
                <button type="button" className="btn btn-quiet btn-compact" onClick={() => void nav("close")}>
                  关闭窗口
                </button>
              </div>
              <div style={{ display: "flex", gap: 6, marginTop: 8, flexWrap: "wrap" }}>
                {QUICK_SITES.map((site) => (
                  <button
                    key={site.url}
                    type="button"
                    className="btn btn-quiet btn-compact"
                    onClick={() => void openBrowser(site.url)}
                  >
                    {site.label}
                  </button>
                ))}
              </div>
            </div>
          </section>

          <section className="release-section">
            <div className="release-row-text">
              <strong>作品页网址</strong>
              <p className="queue-hint">
                exhentai 未登录一定是白屏（地址会变成 <code>?poni=no</code>）。先点「登录 e-hentai」登录，
                再回到 exhentai 打开作品页，然后回来点下面这个按钮。
              </p>
              <input
                value={url}
                onChange={(event) => setUrl(event.target.value)}
                placeholder="https://exhentai.org/g/.../"
                spellCheck={false}
                style={{
                  width: "100%",
                  marginTop: 8,
                  padding: "8px 10px",
                  borderRadius: 6,
                  border: "1px solid var(--border-strong)",
                  background: "var(--panel-raised)",
                  color: "var(--text)",
                }}
              />
            </div>
          </section>

          <section className="release-section">
            <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
              <button type="button" className="btn btn-primary" disabled={busy} onClick={() => void run()}>
                {busy ? "抓取中…" : "开始抓取并入库"}
              </button>
              {busy && (
                <button type="button" className="btn btn-quiet" onClick={() => void cancel()}>
                  取消
                </button>
              )}
            </div>
            {busy && status && <p className="queue-hint">{status}</p>}
            {!busy && !error && (
              <p className="queue-hint">
                抓取目前覆盖 exhentai / e-hentai 作品页；其余站点能正常浏览，只是还没有解析器。
              </p>
            )}
            {error && (
              <p className="queue-hint" style={{ color: "var(--danger, #d9534f)" }}>
                {error}
              </p>
            )}
          </section>

          {report && (
            <section className="release-section">
              <div className="release-row-text">
                <strong>本次结果</strong>
                <p className="queue-hint">标题：{report.title}</p>
                <p className="queue-hint">入库：{report.importReport.message}</p>
                <p className="queue-hint">
                  新增 {report.importReport.imported} / 跳过 {report.importReport.skipped} / 失败{" "}
                  {report.importReport.failed}
                </p>
                <p className="release-path">CBZ：{report.cbzPath}</p>
                <p className="release-path">来源：{report.sourceUrl}</p>
                <p className="release-path">图片：{report.imageUrl}</p>
              </div>
            </section>
          )}
        </div>
      </div>
    </div>
  );
}
