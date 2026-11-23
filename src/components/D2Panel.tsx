import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { api } from "../api";
import type { D2Done, D2FetchReport, GmScriptMeta } from "../types";
import { PlusIcon, TrashIcon } from "./Icons";

interface D2PanelProps {
  onClose: () => void;
  onNotify: (message: string, kind?: "info" | "success" | "error") => void;
}

/** 常用入口。只是快捷方式，不是白名单——地址栏可以填任何站点。 */
const QUICK_SITES: { label: string; url: string; hint?: string }[] = [
  { label: "登录 e-hentai", url: "https://e-hentai.org/" },
  { label: "论坛登录", url: "https://forums.e-hentai.org/index.php?act=Login&CODE=01" },
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
  /** Cookie 登录：粘贴 Cookie 头或一行一个 name=value。 */
  const [cookieText, setCookieText] = useState("");
  const [cookieBusy, setCookieBusy] = useState(false);
  const [tagsBusy, setTagsBusy] = useState(false);
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState("");
  const [error, setError] = useState("");
  const [report, setReport] = useState<D2FetchReport | null>(null);
  const [gmScripts, setGmScripts] = useState<GmScriptMeta[]>([]);
  const [scriptUrl, setScriptUrl] = useState("");
  const [gmBusy, setGmBusy] = useState(false);
  const [gmRemoving, setGmRemoving] = useState<string | null>(null);

  const loadGmScripts = useCallback(async () => {
    try {
      setGmScripts(await api.listGmScripts());
    } catch {
      // 列表拉不到不影响其他功能
    }
  }, []);

  useEffect(() => {
    void loadGmScripts();
  }, [loadGmScripts]);

  /** 从网址安装 .user.js（curl 直连，走系统代理）。 */
  async function installFromUrl() {
    const url = scriptUrl.trim();
    if (!url) {
      onNotify("先填脚本网址", "error");
      return;
    }
    if (gmBusy) return;
    setGmBusy(true);
    try {
      const meta = await api.addGmScriptFromUrl(url);
      setScriptUrl("");
      onNotify(`已安装「${meta.name}」，重开浏览窗口生效`, "success");
      await loadGmScripts();
    } catch (error) {
      onNotify(`安装失败：${String(error)}`, "error");
    } finally {
      setGmBusy(false);
    }
  }

  /** 从本地 .user.js 文件安装。 */
  async function installFromFile() {
    if (gmBusy) return;
    const picked = await open({
      multiple: false,
      filters: [{ name: "油猴脚本", extensions: ["user.js", "js"] }],
    });
    if (typeof picked !== "string") return;
    setGmBusy(true);
    try {
      const meta = await api.addGmScriptFromPath(picked);
      onNotify(`已安装「${meta.name}」，重开浏览窗口生效`, "success");
      await loadGmScripts();
    } catch (error) {
      onNotify(`安装失败：${String(error)}`, "error");
    } finally {
      setGmBusy(false);
    }
  }

  async function toggleScript(script: GmScriptMeta) {
    try {
      await api.setGmScriptEnabled(script.id, !script.enabled);
      await loadGmScripts();
    } catch (error) {
      onNotify(`切换失败：${String(error)}`, "error");
    }
  }

  async function removeScript(script: GmScriptMeta) {
    if (gmRemoving !== script.id) {
      setGmRemoving(script.id);
      window.setTimeout(
        () => setGmRemoving((current) => (current === script.id ? null : current)),
        3200,
      );
      return;
    }
    setGmRemoving(null);
    try {
      await api.removeGmScript(script.id);
      onNotify(`已删除「${script.name}」`, "success");
      await loadGmScripts();
    } catch (error) {
      onNotify(`删除失败：${String(error)}`, "error");
    }
  }

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

  /** 把粘贴的 Cookie 写进内建浏览器（exhentai 登录失败时的主力方案）。 */
  async function applyCookies() {
    const raw = cookieText.trim();
    if (!raw || cookieBusy) return;
    setCookieBusy(true);
    try {
      const written = await api.d2SetCookies(raw);
      onNotify(
        `已写入 ${written} 条 cookie 并刷新浏览窗口；若仍是白页，检查是否漏了 ipb_member_id / ipb_pass_hash / sk`,
        "success",
      );
    } catch (error) {
      onNotify(`写入 cookie 失败：${String(error)}`, "error");
    } finally {
      setCookieBusy(false);
    }
  }

  /** 一键抓取当前页的全部 tag：结果进剪贴板，去导入面板直接粘。 */
  async function collectTags() {
    if (tagsBusy) return;
    setTagsBusy(true);
    try {
      const text = await api.d2CollectTags();
      const count = text.split("\n").filter((line) => line.trim()).length;
      await navigator.clipboard.writeText(text);
      onNotify(`已抓取 ${count} 个标签并复制到剪贴板，去导入面板粘贴即可`, "success");
    } catch (error) {
      onNotify(`抓取标签失败：${String(error)}`, "error");
    } finally {
      setTagsBusy(false);
    }
  }

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

  /** 把浏览窗口当前地址一键填进作品页输入框，省去来回复制粘贴。 */
  async function grabCurrent() {
    try {
      const current = await api.d2CurrentUrl();
      if (!current) {
        onNotify("浏览窗口还没打开，先点上面的「打开」", "info");
        return;
      }
      setUrl(current);
      onNotify("已填入当前页地址", "success");
    } catch (err) {
      onNotify(String(err), "error");
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
              <strong>浏览器</strong>
              <p className="queue-hint">
                多标签浏览器：地址栏直接改网址回车即走；站点弹窗自动开成新标签；无域名限制，窗口和主程序共用登录态。
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
              <strong>油猴脚本（.user.js）</strong>
              <p className="queue-hint">
                Chrome/Edge 生态的 Tampermonkey 兼容子集：@match / @include /
                @exclude / @run-at + GM_* 常用 API。改动脚本后
                <strong>重开浏览窗口</strong>生效。
              </p>
              <div style={{ display: "flex", gap: 6, marginTop: 8 }}>
                <input
                  value={scriptUrl}
                  onChange={(event) => setScriptUrl(event.target.value)}
                  onKeyDown={(event) => {
                    if (event.key === "Enter") void installFromUrl();
                  }}
                  placeholder="https://.../xxx.user.js"
                  spellCheck={false}
                  style={{
                    flex: 1,
                    padding: "7px 10px",
                    borderRadius: 6,
                    border: "1px solid var(--border-strong)",
                    background: "var(--panel-raised)",
                    color: "var(--text)",
                  }}
                />
                <button
                  type="button"
                  className="btn btn-quiet btn-compact"
                  onClick={() => void installFromUrl()}
                  disabled={gmBusy}
                >
                  从网址安装
                </button>
                <button
                  type="button"
                  className="btn btn-quiet btn-compact"
                  onClick={() => void installFromFile()}
                  disabled={gmBusy}
                >
                  <PlusIcon width={13} height={13} />
                  从文件
                </button>
              </div>
              {gmScripts.length > 0 && (
                <ul style={{ margin: "8px 0 0", padding: 0, listStyle: "none" }}>
                  {gmScripts.map((script) => (
                    <li
                      key={script.id}
                      style={{
                        display: "flex",
                        alignItems: "center",
                        gap: 8,
                        padding: "5px 0",
                        borderTop: "1px solid var(--border, #2a2d36)",
                        fontSize: 12,
                      }}
                    >
                      <label
                        style={{ display: "flex", alignItems: "center", gap: 5 }}
                        title={script.enabled ? "已启用（点击停用）" : "已停用（点击启用）"}
                      >
                        <input
                          type="checkbox"
                          checked={script.enabled}
                          onChange={() => void toggleScript(script)}
                        />
                        <strong>{script.name}</strong>
                      </label>
                      {script.version && (
                        <span style={{ color: "var(--faint)" }}>v{script.version}</span>
                      )}
                      <span className="queue-hint" style={{ flex: 1, overflow: "hidden" }}>
                        {(script.matches[0] || script.includes[0] || "全站生效")}
                      </span>
                      <button
                        type="button"
                        className={`btn btn-compact${
                          gmRemoving === script.id ? " btn-danger" : " btn-quiet"
                        }`}
                        onClick={() => void removeScript(script)}
                      >
                        <TrashIcon width={13} height={13} />
                        {gmRemoving === script.id ? "确认" : "删"}
                      </button>
                    </li>
                  ))}
                </ul>
              )}
            </div>
          </section>

          <section className="release-section">
            <div className="release-row-text">
              <strong>Cookie 登录（推荐）</strong>
              <p className="queue-hint">
                在正常浏览器里登录 exhentai 后按 <code>F12</code> → Application → Cookies，
                复制 <code>ipb_member_id</code>、<code>ipb_pass_hash</code>、<code>sk</code> 三项；
                或直接整段复制 Cookie 请求头。一行一个、或分号分隔都能识别。
              </p>
              <textarea
                className="import-tag-input d2-cookie-input"
                rows={3}
                value={cookieText}
                spellCheck={false}
                placeholder={"ipb_member_id=123456\nipb_pass_hash=abcdef...\nsk=xyz..."}
                onChange={(event) => setCookieText(event.target.value)}
              />
              <div className="d2-cookie-actions">
                <button
                  type="button"
                  className="btn btn-primary btn-compact"
                  disabled={!cookieText.trim() || cookieBusy}
                  onClick={() => void applyCookies()}
                >
                  {cookieBusy ? "写入中…" : "应用 Cookie 并刷新"}
                </button>
                <button
                  type="button"
                  className="btn btn-quiet btn-compact"
                  onClick={() => setCookieText("")}
                >
                  清空输入
                </button>
              </div>
              <div className="d2-cookie-actions">
                <button
                  type="button"
                  className="btn btn-primary btn-compact"
                  disabled={tagsBusy}
                  onClick={() => void collectTags()}
                  title="在当前打开的标签页里抓全部 /tag/ 链接，整理后进剪贴板"
                >
                  {tagsBusy ? "抓取中…" : "一键抓取本页标签"}
                </button>
                <span className="queue-hint">
                  在作品详情页点它，几百个标签一次进剪贴板；普通页面会提示没有 tag。
                </span>
              </div>
              <p className="queue-hint">
                只写到浏览窗口的 cookie 库（e-hentai 与 exhentai 共用），不保存到任何文件；
                想退出这个会话，清空浏览器数据或换一组 cookie 再点一次。
              </p>
            </div>
          </section>

          <section className="release-section">
            <div className="release-row-text">
              <strong>作品页网址</strong>
              <p className="queue-hint">
                exhentai 白屏 = 未登录或账号无 exh 权限（地址会变成 <code>?poni=no</code>）。正确顺序：
                点「登录 e-hentai」在主站登录（右上角 Log in）→ 再开 exhentai.org。
                登录完成后回到这里填作品页网址。系统代理必须开着。
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
              <div style={{ display: "flex", gap: 6, marginTop: 8 }}>
                <button type="button" className="btn btn-quiet btn-compact" onClick={() => void grabCurrent()}>
                  抓取当前页
                </button>
                <span className="queue-hint" style={{ alignSelf: "center" }}>
                  直接取浏览窗口当前地址，不用再复制一遍
                </span>
              </div>
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
                <p className="queue-hint">
                  抓取 {report.pages} 张 / 跳过 {report.failed} 张
                </p>
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
