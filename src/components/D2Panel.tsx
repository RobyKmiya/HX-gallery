import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../api";
import type { D2Done, D2FetchReport } from "../types";

interface D2PanelProps {
  onClose: () => void;
  onNotify: (message: string, kind?: "info" | "success" | "error") => void;
}

/**
 * D2 M1 的临时入口。
 *
 * 打开 exhentai 作品页 → 抓第一页 → 打包 CBZ → 复用 importer 入库。
 *
 * 注意：这里**故意不等长任务**。早期版本 `await api.d2FetchFirst()` 会挂到
 * 60 秒超时才返回，期间界面只剩一个「抓取中…」，管理员实测就是「点击直接卡死」。
 * 现在改成 `d2Start` 立即返回 + 事件回报，任何时候都能取消。
 */
export default function D2Panel({ onClose, onNotify }: D2PanelProps) {
  const [url, setUrl] = useState("");
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState("");
  const [error, setError] = useState("");
  const [report, setReport] = useState<D2FetchReport | null>(null);

  useEffect(() => {
    const unlistenStatus = listen<{ message: string }>("d2://status", (event) => {
      setStatus(event.payload.message);
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
      void unlistenDone.then((unlisten) => unlisten());
    };
  }, [onNotify]);

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
              <strong>作品页网址</strong>
              <p className="queue-hint">
                当前仅支持 exhentai.org / e-hentai.org。点击后会在弹窗中打开站点；请先登录，再导航到作品详情页（地址含 /g/），画匣会自动抓取第一页。
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
              <button
                type="button"
                className="btn btn-primary"
                disabled={busy}
                onClick={() => void run()}
              >
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
                M1 只抓第一页，打包成单页 CBZ 后调用现有 importer 入库；暂存文件保留在缓存目录。
              </p>
            )}
            {error && <p className="queue-hint" style={{ color: "var(--danger, #d9534f)" }}>{error}</p>}
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
