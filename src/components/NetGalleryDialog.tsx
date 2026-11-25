import { useEffect, useRef, useState } from "react";
import { api } from "../api";
import { notifyModalOpen, notifyModalClose } from "../modalBus";
import { CloseIcon } from "./Icons";

/**
 * 网络画集下载：粘贴作品页链接 → 解析标题/标签/页数 → 下载整本并入库。
 *
 * 管理员 2026-11-25 要求：能在画匣里直接按链接下书，别再开浏览器插件绕一圈。
 * 下载走 Rust 侧的 curl（复用主窗 cookie），标签随作品页一起抓，入库后
 * 在详情抽屉里就能看到。
 */
interface NetGalleryDialogProps {
  onClose: () => void;
  /** 下载完成回调：让外层刷新库列表。 */
  onDone?: () => void;
}

interface NetDlStatus {
  running: boolean;
  finished: boolean;
  error: string;
  stage: string;
  done: number;
  total: number;
  title: string;
  tags: [string, string][];
  path: string;
}

const EMPTY: NetDlStatus = {
  running: false,
  finished: false,
  error: "",
  stage: "",
  done: 0,
  total: 0,
  title: "",
  tags: [],
  path: "",
};

export default function NetGalleryDialog({ onClose, onDone }: NetGalleryDialogProps) {
  const [url, setUrl] = useState("");
  const [status, setStatus] = useState<NetDlStatus>(EMPTY);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const timerRef = useRef<number | null>(null);

  useEffect(() => {
    notifyModalOpen();
    return () => {
      notifyModalClose();
      if (timerRef.current !== null) window.clearInterval(timerRef.current);
    };
  }, []);

  const stopPolling = () => {
    if (timerRef.current !== null) {
      window.clearInterval(timerRef.current);
      timerRef.current = null;
    }
  };

  const startPolling = () => {
    stopPolling();
    timerRef.current = window.setInterval(async () => {
      try {
        const next = await api.netDlStatus();
        setStatus(next);
        if (!next.running) {
          stopPolling();
          setBusy(false);
          if (next.finished) onDone?.();
        }
      } catch {
        stopPolling();
        setBusy(false);
      }
    }, 700);
  };

  const preview = async () => {
    const target = url.trim();
    if (!target) {
      setError("先把作品页链接贴进来");
      return;
    }
    setError("");
    setBusy(true);
    try {
      const info = await api.netDlPreview(target);
      setStatus(info);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const start = async () => {
    const target = url.trim();
    if (!target) {
      setError("先把作品页链接贴进来");
      return;
    }
    setError("");
    setBusy(true);
    setStatus({ ...EMPTY, running: true, stage: "准备中…" });
    try {
      await api.netDlStart(target);
      startPolling();
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };

  const cancel = async () => {
    try {
      await api.netDlCancel();
    } catch {
      /* 取消失败不影响轮询收尾 */
    }
  };

  const percent =
    status.total > 0 ? Math.min(100, Math.round((status.done / status.total) * 100)) : 0;

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="modal net-dl-dialog"
        style={{ width: "min(620px, 92vw)" }}
        onClick={(event) => event.stopPropagation()}
      >
        <header className="modal-title-row">
          <div>
            <h2>网络画集</h2>
            <p className="modal-hint">
              贴作品页链接（exhentai / e-hentai），抓全书图片和标签，打包进画匣。
            </p>
          </div>
          <button type="button" className="icon-button" onClick={onClose} aria-label="关闭">
            <CloseIcon />
          </button>
        </header>

        <div style={{ padding: "0 18px 8px" }}>
          <input
            type="text"
            value={url}
            onChange={(event) => setUrl(event.target.value)}
            placeholder="https://exhentai.org/g/1234567/abcdef1234/"
            spellCheck={false}
            style={{
              width: "100%",
              padding: "9px 11px",
              borderRadius: 8,
              border: "1px solid var(--line, #2a2a33)",
              background: "var(--panel-2, #16161c)",
              color: "var(--text, #e6e6ea)",
              fontSize: 13,
            }}
            onKeyDown={(event) => {
              if (event.key === "Enter") void preview();
            }}
          />
          <p style={{ margin: "8px 0 0", fontSize: 12, color: "var(--text-dim, #9a9aa4)" }}>
            提示：需要先在内置浏览器里登录该站点，下载会沿用那套 cookie。
          </p>
        </div>

        {status.title || status.total > 0 ? (
          <div style={{ padding: "4px 18px 0" }}>
            <p style={{ margin: "6px 0", fontSize: 14, fontWeight: 600 }}>
              {status.title || "（未识别到标题）"}
              {status.total > 0 ? ` · ${status.total} 页` : ""}
            </p>
            {status.tags.length > 0 ? (
              <div
                style={{
                  display: "flex",
                  flexWrap: "wrap",
                  gap: 6,
                  maxHeight: 120,
                  overflow: "auto",
                  marginBottom: 6,
                }}
              >
                {status.tags.slice(0, 60).map(([namespace, value]) => (
                  <span
                    key={`${namespace}:${value}`}
                    style={{
                      fontSize: 11,
                      padding: "2px 7px",
                      borderRadius: 999,
                      background: "var(--panel-2, #1b1b22)",
                      border: "1px solid var(--line, #2a2a33)",
                      color: "var(--text-dim, #b6b6bf)",
                    }}
                  >
                    {namespace}: {value}
                  </span>
                ))}
              </div>
            ) : null}
          </div>
        ) : null}

        {status.running || status.finished ? (
          <div style={{ padding: "8px 18px 0" }}>
            <div
              style={{
                height: 6,
                borderRadius: 999,
                background: "var(--panel-2, #1b1b22)",
                overflow: "hidden",
              }}
            >
              <div
                style={{
                  width: `${status.finished ? 100 : percent}%`,
                  height: "100%",
                  background: "var(--accent, #6ea8fe)",
                  transition: "width .25s ease",
                }}
              />
            </div>
            <p style={{ margin: "7px 0 0", fontSize: 12, color: "var(--text-dim, #9a9aa4)" }}>
              {status.stage}
              {status.total > 0 ? `（${status.done}/${status.total}）` : ""}
            </p>
          </div>
        ) : null}

        {error ? (
          <p style={{ margin: "10px 18px 0", fontSize: 12, color: "#ff8080" }}>{error}</p>
        ) : null}
        {status.error ? (
          <p style={{ margin: "10px 18px 0", fontSize: 12, color: "#ff8080" }}>{status.error}</p>
        ) : null}

        <footer className="modal-actions" style={{ padding: "14px 18px 18px" }}>
          <button type="button" className="btn" onClick={onClose}>
            关闭
          </button>
          {status.running ? (
            <button type="button" className="btn" onClick={() => void cancel()}>
              取消下载
            </button>
          ) : (
            <button type="button" className="btn" disabled={busy} onClick={() => void preview()}>
              解析
            </button>
          )}
          <button
            type="button"
            className="btn btn-primary"
            disabled={busy || status.running}
            onClick={() => void start()}
          >
            {status.running ? "下载中…" : "下载并入库"}
          </button>
        </footer>
      </div>
    </div>
  );
}
