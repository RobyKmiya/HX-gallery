import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { api } from "../api";
import type { D2Done, D2FetchReport, GmScriptMeta } from "../types";
import { PlusIcon, TrashIcon } from "./Icons";

interface D2PanelProps {
  onClose: () => void;
  onNotify: (message: string, kind?: "info" | "success" | "error") => void;
  /** 库目录路径（抓取入库后一键定位用）。 */
  libraryPath?: string;
}

/** Cookie 管理的站点预设（管理员 11-23：常用站的登录方式各不相同）。 */
const COOKIE_SITES: {
  key: string;
  label: string;
  domains: string[];
  hint: string;
}[] = [
  {
    key: "eh",
    label: "exhentai / e-hentai",
    domains: [".e-hentai.org", ".exhentai.org"],
    hint: "必填 ipb_member_id、ipb_pass_hash、sk（少 sk 必白页）",
  },
  {
    key: "18comic",
    label: "18comic（禁漫天堂）",
    domains: [".18comic.vip", ".18comic.org"],
    hint: "登录后复制 AVS 相关 cookie，或整段 Cookie 头",
  },
  {
    key: "wnacg",
    label: "wnacg（绅士漫画）",
    domains: [".wnacg.com"],
    hint: "登录态 cookie，通常一两个就够",
  },
  {
    key: "nhentai",
    label: "nhentai",
    domains: [".nhentai.net"],
    hint: "登录态 cookie（csrftoken / sessionid）",
  },
];

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
export default function D2Panel({ onClose, onNotify, libraryPath }: D2PanelProps) {
  const [browseUrl, setBrowseUrl] = useState("https://e-hentai.org/");
  const [url, setUrl] = useState("");
  const [tagsBusy, setTagsBusy] = useState(false);
  /** 各站 cookie 草稿（站点 key → 文本），持久化在 localStorage（明文，本机自用）。 */
  const [cookieDrafts, setCookieDrafts] = useState<Record<string, string>>(() => {
    try {
      return JSON.parse(window.localStorage.getItem("huaxia.d2.cookieDrafts") || "{}");
    } catch {
      return {};
    }
  });
  const [cookieSavedAt, setCookieSavedAt] = useState<Record<string, number>>(() => {
    try {
      return JSON.parse(window.localStorage.getItem("huaxia.d2.cookieSavedAt") || "{}");
    } catch {
      return {};
    }
  });
  const [cookieBusyKey, setCookieBusyKey] = useState<string | null>(null);
  const [adblockOn, setAdblockOn] = useState(() => {
    try {
      return window.localStorage.getItem("huaxia.d2.adblock") !== "0";
    } catch {
      return true;
    }
  });
  const [homeDraft, setHomeDraft] = useState(() => {
    try {
      return window.localStorage.getItem("huaxia.d2.home") || "https://e-hentai.org/";
    } catch {
      return "https://e-hentai.org/";
    }
  });
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

  /** 浏览器主页：设置里改，打开浏览器不带地址时用它。 */
  function getHome(): string {
    try {
      return window.localStorage.getItem("huaxia.d2.home") || "https://e-hentai.org/";
    } catch {
      return "https://e-hentai.org/";
    }
  }

  function saveCookieDraft(key: string, text: string) {
    const nextDrafts = { ...cookieDrafts, [key]: text };
    setCookieDrafts(nextDrafts);
    try {
      window.localStorage.setItem("huaxia.d2.cookieDrafts", JSON.stringify(nextDrafts));
    } catch {
      // 存不了就只在本次生效
    }
  }

  /** 把某站的 cookie 写进内建浏览器（只写该站的域名）。 */
  async function applySiteCookies(site: (typeof COOKIE_SITES)[number]) {
    const raw = (cookieDrafts[site.key] ?? "").trim();
    if (!raw || cookieBusyKey) return;
    setCookieBusyKey(site.key);
    try {
      const written = await api.d2SetCookies(raw, site.domains);
      const savedAt = { ...cookieSavedAt, [site.key]: Date.now() };
      setCookieSavedAt(savedAt);
      try {
        window.localStorage.setItem("huaxia.d2.cookieSavedAt", JSON.stringify(savedAt));
      } catch {
        // 同上
      }
      onNotify(
        `已写入 ${written} 条 cookie 到 ${site.label} 并刷新浏览窗口`,
        "success",
      );
    } catch (error) {
      onNotify(`写入 cookie 失败：${String(error)}`, "error");
    } finally {
      setCookieBusyKey(null);
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
    let next = (target ?? browseUrl).trim();
    if (!next) {
      // 没填地址：用设置里的主页（默认 e-hentai）
      next = getHome();
    }
    setBrowseUrl(next);
    setError("");
    try {
      await api.d2Open(next, adblockOn);
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
              <strong>浏览器设置</strong>
              <p className="queue-hint">
                主页：在 D2 面板点「打开浏览器」不填地址时用它。
                扩展说明：内嵌 WebView2 <strong>不支持安装浏览器扩展</strong>（AdGuard 等
                Edge 商店扩展装不了）；去广告可改用油猴脚本路线。
              </p>
              <input
                value={homeDraft}
                onChange={(event) => setHomeDraft(event.target.value)}
                placeholder="https://e-hentai.org/"
                spellCheck={false}
                style={{
                  width: "100%",
                  marginTop: 8,
                  padding: "7px 10px",
                  borderRadius: 6,
                  border: "1px solid var(--border-strong)",
                  background: "var(--panel-raised)",
                  color: "var(--text)",
                }}
              />
              <div className="d2-cookie-actions">
                <button
                  type="button"
                  className="btn btn-primary btn-compact"
                  onClick={() => {
                    try {
                      window.localStorage.setItem("huaxia.d2.home", homeDraft.trim());
                      onNotify("主页已保存", "success");
                    } catch {
                      onNotify("主页保存失败（存储被禁用？）", "error");
                    }
                  }}
                >
                  保存主页
                </button>
              </div>
              <label className="settings-row" style={{ marginTop: 10 }}>
                <span>广告拦截（内置名单 + 自定义文件，重开浏览窗口生效）</span>
                <input
                  type="checkbox"
                  checked={adblockOn}
                  onChange={(event) => {
                    const next = event.target.checked;
                    setAdblockOn(next);
                    try {
                      window.localStorage.setItem("huaxia.d2.adblock", next ? "1" : "0");
                    } catch {
                      // 存不了就只在本次生效
                    }
                  }}
                />
              </label>
              <p className="queue-hint">
                在 WebView2 网络请求层拦域名（机制同 AdGuard 的网络过滤）：
                内置 40+ 个广告/弹窗/统计网络（exoclick、popads、propellerads、hilltopads、
                doubleclick 等）。想追加域名，往
                <code>%LOCALAPPDATA%\com.archivegallery.desktop\browser\adblock.txt</code>
                一行一个写即可（# 开头是注释）。隐藏不了的「页面内广告元素」属于装饰性过滤，
                可以再写油猴脚本处理。
              </p>
            </div>
          </section>

          <section className="release-section">
            <div className="release-row-text">
              <strong>站点 Cookie 管理</strong>
              <p className="queue-hint">
                在你常用的浏览器里登录各站点，按 <code>F12</code> → Application → Cookies
                复制需要的项粘到下面（一行一个，或整段 Cookie 头都行）。
                Cookie 只存本机 localStorage（明文），写进内建浏览器后即记住登录态；
                WebView2 的 cookie 库本身跨重启保留。
              </p>
              <div className="d2-cookie-actions" style={{ marginBottom: 10 }}>
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
              {COOKIE_SITES.map((site) => {
                const saved = cookieSavedAt[site.key];
                return (
                  <div className="d2-site-row" key={site.key}>
                    <div className="d2-site-head">
                      <strong>{site.label}</strong>
                      {saved ? (
                        <span className="queue-hint">
                          上次应用 {new Date(saved).toLocaleString()}
                        </span>
                      ) : (
                        <span className="queue-hint">未应用</span>
                      )}
                    </div>
                    <p className="queue-hint">{site.hint}</p>
                    <textarea
                      className="import-tag-input d2-cookie-input"
                      rows={2}
                      value={cookieDrafts[site.key] ?? ""}
                      spellCheck={false}
                      placeholder={"name1=value1\nname2=value2"}
                      onChange={(event) => saveCookieDraft(site.key, event.target.value)}
                    />
                    <div className="d2-cookie-actions">
                      <button
                        type="button"
                        className="btn btn-primary btn-compact"
                        disabled={!(cookieDrafts[site.key] ?? "").trim() || cookieBusyKey !== null}
                        onClick={() => void applySiteCookies(site)}
                      >
                        {cookieBusyKey === site.key ? "写入中…" : "写入浏览器"}
                      </button>
                    </div>
                  </div>
                );
              })}
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
                placeholder="https://exhentai.org/g/.../  ·  nhentai.net/g/.../  ·  18comic.vip/album/...  ·  wnacg.com/photos-index-aid-....html"
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
                  填入当前页地址
                </button>
                <span className="queue-hint" style={{ alignSelf: "center" }}>
                  只把地址填进来；真要抓整部，用浏览器工具栏上的 ⬇ 按钮
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
                抓取支持 exhentai / e-hentai / nhentai / 18comic / wnacg 作品页；其余站点能正常浏览，只是还没有解析器。
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
                {libraryPath && (
                  <button
                    type="button"
                    className="btn btn-quiet btn-compact"
                    style={{ marginTop: 6 }}
                    onClick={() => void api.revealInExplorer(libraryPath)}
                    title={`在资源管理器打开 ${libraryPath}`}
                  >
                    打开库目录看文件
                  </button>
                )}
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
