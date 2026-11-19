import { useState } from "react";
import { api } from "../api";
import type { D2FetchReport } from "../types";

interface D2PanelProps {
  onClose: () => void;
  onNotify: (message: string, kind?: "info" | "success" | "error") => void;
}

/**
 * D2 M1 的临时入口。
 *
 * 打开 exhentai 作品页 → 抓第一页 → 打包 CBZ → 复用 importer 入库。
 * 正式 UI 会在 M5 重做，这里先把闭环打通并保留可观察的结果。
 */
export default function D2Panel({ onClose, onNotify }: D2PanelProps) {
  const [url, setUrl] = useState("");
  const [busy, setBusy] = useState(false);
  const [report, setReport] = useState<D2FetchReport | null>(null);

  async function run() {
    if (!url.trim()) {
      onNotify("请填写作品页网址", "error");
      return;
    }
    setBusy(true);
    setReport(null);
    try {
      const result = await api.d2FetchFirst(url.trim());
      setReport(result);
      onNotify(`D2 抓取完成：${result.title}`, "success");
    } catch (error) {
      onNotify(`D2 抓取失败：${String(error)}`, "error");
    } finally {
      setBusy(false);
    }
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
            <button
              type="button"
              className="btn btn-primary"
              disabled={busy}
              onClick={() => void run()}
            >
              {busy ? "抓取中…" : "开始抓取并入库"}
            </button>
            <p className="queue-hint">
              M1 只抓第一页，打包成单页 CBZ 后调用现有 importer 入库；暂存文件保留在缓存目录。
            </p>
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
