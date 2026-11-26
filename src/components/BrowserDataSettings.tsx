import { useEffect, useState } from "react";
import { api } from "../api";
import type { CredentialEntry, HistoryEntry, SiteLoginEntry } from "../types";

function formatTime(ms: number): string {
  if (!ms) return "";
  return new Date(ms).toLocaleString();
}

interface Draft {
  id: number | null;
  site: string;
  username: string;
  password: string;
  note: string;
}

/** 常见的几个图站：直接给一个粘贴框，不用每次手打域名。 */
const COOKIE_SITES: { host: string; label: string; hint: string }[] = [
  {
    host: "18comic.vip",
    label: "18comic（禁漫天堂）",
    hint: "登录后复制 document.cookie，或整段 Cookie 请求头；关键是 AVS",
  },
  {
    host: "nhentai.net",
    label: "nhentai",
    hint: "csrftoken / access_token / sessionid",
  },
  {
    host: "exhentai.org",
    label: "exhentai / e-hentai",
    hint: "ipb_member_id / ipb_pass_hash / sk / igneous",
  },
  {
    host: "wnacg.com",
    label: "wnacg（绅士漫画）",
    hint: "登录态 cookie，通常一两条就够",
  },
];

/**
 * 设置 → 浏览器数据：浏览历史 + 账号密码。
 *
 * 密码在库里是 DPAPI 密文，列表不带明文；点「显示密码」才单独解密取一次。
 * 密码只在本机当前 Windows 用户下能解，拷走 DB 也读不出来。
 */
export default function BrowserDataSettings() {
  const [tab, setTab] = useState<"history" | "credential" | "siteLogin">("history");
  const [hint, setHint] = useState("");
  const [busy, setBusy] = useState(false);

  const [history, setHistory] = useState<HistoryEntry[]>([]);
  const [search, setSearch] = useState("");

  const [creds, setCreds] = useState<CredentialEntry[]>([]);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [revealed, setRevealed] = useState<Record<number, string>>({});

  const [logins, setLogins] = useState<SiteLoginEntry[]>([]);
  const [loginHost, setLoginHost] = useState("");
  const [loginNote, setLoginNote] = useState("");
  const [cookieText, setCookieText] = useState("");
  const [siteDrafts, setSiteDrafts] = useState<Record<string, string>>({});
  const [busySite, setBusySite] = useState("");

  const loadHistory = async (keyword = "") => {
    try {
      setHistory(await api.browserHistoryList(keyword || undefined, 300));
    } catch (error) {
      setHint(`历史读取失败：${String(error)}`);
    }
  };

  const loadCreds = async () => {
    try {
      setCreds(await api.browserCredentialList());
    } catch (error) {
      setHint(`账号读取失败：${String(error)}`);
    }
  };

  const loadLogins = async () => {
    try {
      setLogins(await api.siteLoginList());
    } catch (error) {
      setHint(`登录态读取失败：${String(error)}`);
    }
  };

  useEffect(() => {
    void loadHistory();
    void loadCreds();
    void loadLogins();
  }, []);

  const clearHistory = async () => {
    if (!window.confirm("确定清空全部浏览历史？")) return;
    setBusy(true);
    try {
      await api.browserHistoryClear();
      setHistory([]);
      setHint("浏览历史已清空");
    } catch (error) {
      setHint(`清空失败：${String(error)}`);
    } finally {
      setBusy(false);
    }
  };

  const removeHistory = async (id: number) => {
    try {
      await api.browserHistoryDelete(id);
      setHistory((prev) => prev.filter((item) => item.id !== id));
    } catch (error) {
      setHint(`删除失败：${String(error)}`);
    }
  };

  const reveal = async (id: number) => {
    if (revealed[id]) {
      setRevealed((prev) => {
        const next = { ...prev };
        delete next[id];
        return next;
      });
      return;
    }
    try {
      const password = await api.browserCredentialReveal(id);
      setRevealed((prev) => ({ ...prev, [id]: password }));
    } catch (error) {
      setHint(`解密失败：${String(error)}`);
    }
  };

  const saveDraft = async () => {
    if (!draft) return;
    if (!draft.site.trim() || !draft.username.trim()) {
      setHint("站点和用户名不能为空");
      return;
    }
    setBusy(true);
    try {
      await api.browserCredentialSave({
        id: draft.id,
        site: draft.site,
        username: draft.username,
        password: draft.password ? draft.password : null,
        note: draft.note,
      });
      setDraft(null);
      setHint("账号已保存");
      await loadCreds();
    } catch (error) {
      setHint(`保存失败：${String(error)}`);
    } finally {
      setBusy(false);
    }
  };

  const removeCred = async (id: number) => {
    if (!window.confirm("删除这条账号？")) return;
    try {
      await api.browserCredentialDelete(id);
      await loadCreds();
    } catch (error) {
      setHint(`删除失败：${String(error)}`);
    }
  };

  const captureLogin = async () => {
    const host = loginHost.trim();
    if (!host) {
      setHint("请先填站点域名，例如 18comic.org");
      return;
    }
    setBusy(true);
    try {
      const count = await api.siteLoginCapture(host, loginNote.trim());
      setHint(`已保存 ${host} 的登录态（${count} 项 Cookie），以后下载会直接沿用`);
      setLoginHost("");
      setLoginNote("");
      await loadLogins();
    } catch (error) {
      setHint(`保存登录态失败：${String(error)}`);
    } finally {
      setBusy(false);
    }
  };

  const importLogin = async () => {
    const host = loginHost.trim();
    if (!host) {
      setHint("请先在上面填站点域名，例如 18comic.vip");
      return;
    }
    if (!cookieText.trim()) {
      setHint("先把 Cookie 粘到下面的框里");
      return;
    }
    setBusy(true);
    try {
      const count = await api.siteLoginImport(host, cookieText);
      setHint(`已把 ${host} 的 ${count} 项 Cookie 存入本机加密库，下载会立刻用它`);
      setCookieText("");
      await loadLogins();
    } catch (error) {
      setHint(`入库失败：${String(error)}`);
    } finally {
      setBusy(false);
    }
  };

  const removeLogin = async (id: number) => {
    if (!window.confirm("删除这条登录态？删除后下载该站可能要重新登录。")) return;
    try {
      await api.siteLoginDelete(id);
      await loadLogins();
    } catch (error) {
      setHint(`删除失败：${String(error)}`);
    }
  };

  /** 快捷站点：把该行粘贴的 Cookie 直接写进加密库。 */
  const importSiteCookie = async (host: string) => {
    const text = (siteDrafts[host] ?? "").trim();
    if (!text) return;
    setBusySite(host);
    try {
      const count = await api.siteLoginImport(host, text);
      setSiteDrafts((prev) => ({ ...prev, [host]: "" }));
      setHint(`${host}：已存入 ${count} 项 Cookie，下载立刻可用`);
      await loadLogins();
    } catch (error) {
      setHint(`${host} 入库失败：${String(error)}`);
    } finally {
      setBusySite("");
    }
  };

  const captureFor = async (host: string) => {
    setBusy(true);
    try {
      const count = await api.siteLoginCapture(host);
      setHint(`已从当前标签抓到 ${host} 的 ${count} 项 Cookie`);
      await loadLogins();
    } catch (error) {
      setHint(`抓取失败：${String(error)}`);
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <h3>浏览器数据</h3>
      <p className="settings-hint">
        浏览历史自动记录（最多 3000 条，同网址 10 分钟内合并）；账号密码和登录态 Cookie
        都用 Windows DPAPI 加密，只在本机当前用户下能解，DB 拷走也读不出来。
        网页标签里登录过的站点会自动存登录态，关掉画匣也不会丢。
      </p>

      <div className="browser-data-tabs">
        <button
          type="button"
          className={tab === "history" ? "is-active" : ""}
          onClick={() => setTab("history")}
        >
          浏览历史
        </button>
        <button
          type="button"
          className={tab === "credential" ? "is-active" : ""}
          onClick={() => setTab("credential")}
        >
          账号密码
        </button>
        <button
          type="button"
          className={tab === "siteLogin" ? "is-active" : ""}
          onClick={() => setTab("siteLogin")}
        >
          登录态
        </button>
      </div>

      {hint && <p className="settings-hint">{hint}</p>}

      {tab === "history" && (
        <>
          <div className="browser-data-toolbar">
            <input
              value={search}
              onChange={(event) => setSearch(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") void loadHistory(search);
              }}
              placeholder="搜索网址或标题"
              className="settings-inline-input"
            />
            <button type="button" className="btn btn-quiet btn-compact" onClick={() => void loadHistory(search)}>
              搜索
            </button>
            <button
              type="button"
              className="btn btn-quiet btn-compact"
              onClick={() => void clearHistory()}
              disabled={busy || history.length === 0}
            >
              清空历史
            </button>
          </div>
          <div className="browser-data-list">
            {history.map((item) => (
              <div className="browser-data-row" key={item.id}>
                <div className="browser-data-main">
                  <span className="browser-data-title">{item.title || item.url}</span>
                  <span className="browser-data-sub">{item.url}</span>
                </div>
                <span className="browser-data-time">{formatTime(item.visitedAt)}</span>
                <button
                  type="button"
                  className="browser-data-remove"
                  onClick={() => void removeHistory(item.id)}
                  title="删除这条"
                >
                  ×
                </button>
              </div>
            ))}
            {history.length === 0 && <p className="settings-hint">还没有历史记录。</p>}
          </div>
        </>
      )}

      {tab === "credential" && (
        <>
          <div className="browser-data-toolbar">
            <button
              type="button"
              className="btn btn-quiet btn-compact"
              onClick={() => setDraft({ id: null, site: "", username: "", password: "", note: "" })}
            >
              新增账号
            </button>
          </div>

          {draft && (
            <div className="browser-data-editor">
              <label className="settings-row">
                <span>站点</span>
                <input
                  value={draft.site}
                  onChange={(event) => setDraft({ ...draft, site: event.target.value })}
                  placeholder="如 e-hentai.org"
                  className="settings-inline-input"
                />
              </label>
              <label className="settings-row">
                <span>用户名</span>
                <input
                  value={draft.username}
                  onChange={(event) => setDraft({ ...draft, username: event.target.value })}
                  className="settings-inline-input"
                />
              </label>
              <label className="settings-row">
                <span>密码</span>
                <input
                  type="password"
                  value={draft.password}
                  onChange={(event) => setDraft({ ...draft, password: event.target.value })}
                  placeholder={draft.id ? "留空 = 不改密码" : ""}
                  className="settings-inline-input"
                />
              </label>
              <label className="settings-row">
                <span>备注</span>
                <input
                  value={draft.note}
                  onChange={(event) => setDraft({ ...draft, note: event.target.value })}
                  placeholder="用途 / 二级验证 / 备用邮箱等"
                  className="settings-inline-input"
                />
              </label>
              <div className="browser-data-toolbar">
                <button
                  type="button"
                  className="btn btn-primary btn-compact"
                  onClick={() => void saveDraft()}
                  disabled={busy}
                >
                  保存
                </button>
                <button type="button" className="btn btn-quiet btn-compact" onClick={() => setDraft(null)}>
                  取消
                </button>
              </div>
            </div>
          )}

          <div className="browser-data-list">
            {creds.map((item) => (
              <div className="browser-data-row" key={item.id}>
                <div className="browser-data-main">
                  <span className="browser-data-title">
                    {item.site} · {item.username}
                  </span>
                  <span className="browser-data-sub">
                    {revealed[item.id] ? `密码：${revealed[item.id]}` : item.note || "（无备注）"}
                  </span>
                </div>
                <button
                  type="button"
                  className="btn btn-quiet btn-compact"
                  onClick={() => void reveal(item.id)}
                >
                  {revealed[item.id] ? "隐藏" : "显示密码"}
                </button>
                <button
                  type="button"
                  className="btn btn-quiet btn-compact"
                  onClick={() =>
                    setDraft({
                      id: item.id,
                      site: item.site,
                      username: item.username,
                      password: "",
                      note: item.note,
                    })
                  }
                >
                  编辑
                </button>
                <button
                  type="button"
                  className="browser-data-remove"
                  onClick={() => void removeCred(item.id)}
                  title="删除这条"
                >
                  ×
                </button>
              </div>
            ))}
            {creds.length === 0 && <p className="settings-hint">还没有账号，点「新增账号」加一条。</p>}
          </div>
          <p className="settings-hint">
            账号簿会在打开对应站点的登录页时自动填入用户名和密码，只填保存过的站点；没有账号的站点不受影响。
          </p>
        </>
      )}

      {tab === "siteLogin" && (
        <>
          <p className="settings-hint">
            网页标签里登录好站点后，这里会自动保存一份加密快照（关掉画匣也不丢）；
            也可以直接把 Cookie 粘进来入库，不必开着标签。下载只读这份快照，
            Cookie 内容不显示、不明文落盘。
          </p>
          <div className="site-cookie-list">
            {COOKIE_SITES.map((site) => {
              const saved = logins.find((item) => item.host === site.host);
              return (
                <div className="site-cookie-row" key={site.host}>
                  <div className="site-cookie-head">
                    <strong>{site.label}</strong>
                    <span className={saved ? "site-cookie-on" : "site-cookie-off"}>
                      {saved ? `已保存 · ${formatTime(saved.updatedAt)}` : "未应用"}
                    </span>
                  </div>
                  <p className="settings-hint">{site.hint}</p>
                  <textarea
                    className="site-login-paste"
                    rows={2}
                    value={siteDrafts[site.host] ?? ""}
                    onChange={(event) =>
                      setSiteDrafts((prev) => ({ ...prev, [site.host]: event.target.value }))
                    }
                    placeholder="name=value; name2=value2"
                  />
                  <div className="browser-data-toolbar">
                    <button
                      type="button"
                      className="btn btn-primary btn-compact"
                      disabled={busySite === site.host || !(siteDrafts[site.host] ?? "").trim()}
                      onClick={() => void importSiteCookie(site.host)}
                    >
                      {busySite === site.host ? "写入中…" : "写入本机库"}
                    </button>
                    <button
                      type="button"
                      className="btn btn-quiet btn-compact"
                      disabled={busy}
                      onClick={() => void captureFor(site.host)}
                      title="从当前打开的网页标签里直接抓一份"
                    >
                      从标签抓取
                    </button>
                  </div>
                </div>
              );
            })}
          </div>
          <p className="settings-hint">其它站点用下面这行手填域名：</p>
          <div className="browser-data-toolbar">
            <input
              value={loginHost}
              onChange={(event) => setLoginHost(event.target.value)}
              placeholder="站点域名，如 18comic.vip"
              className="settings-inline-input"
            />
            <input
              value={loginNote}
              onChange={(event) => setLoginNote(event.target.value)}
              placeholder="备注（可选）"
              className="settings-inline-input"
            />
            <button
              type="button"
              className="btn btn-compact"
              onClick={() => void captureLogin()}
              disabled={busy}
            >
              从当前标签抓取
            </button>
          </div>
          <textarea
            className="site-login-paste"
            value={cookieText}
            onChange={(event) => setCookieText(event.target.value)}
            placeholder={"把 Cookie 粘到这里，支持整段请求头：\nAVS=xxxxxxxx; ipb_member_id=123; ipb_pass_hash=abc…"}
            rows={3}
          />
          <div className="browser-data-toolbar">
            <button
              type="button"
              className="btn btn-primary btn-compact"
              onClick={() => void importLogin()}
              disabled={busy || !cookieText.trim()}
            >
              粘贴 Cookie 入库
            </button>
            <span className="settings-hint">只存本机、DPAPI 加密；下载会立刻用它。</span>
          </div>
          <div className="browser-data-list">
            {logins.map((item) => (
              <div className="browser-data-row" key={item.id}>
                <div className="browser-data-main">
                  <span className="browser-data-title">{item.host}</span>
                  <span className="browser-data-sub">
                    {item.note || "（无备注）"} · {formatTime(item.updatedAt)}
                  </span>
                </div>
                <button
                  type="button"
                  className="browser-data-remove"
                  onClick={() => void removeLogin(item.id)}
                  title="删除这条登录态"
                >
                  ×
                </button>
              </div>
            ))}
            {logins.length === 0 && (
              <p className="settings-hint">还没有保存的登录态。粘贴 Cookie 时也会自动存一份到这里。</p>
            )}
          </div>
        </>
      )}
    </>
  );
}
