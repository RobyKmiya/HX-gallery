import { useEffect, useState } from "react";
import { api } from "../api";
import type { CredentialEntry, HistoryEntry } from "../types";

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

/**
 * 设置 → 浏览器数据：浏览历史 + 账号密码。
 *
 * 密码在库里是 DPAPI 密文，列表不带明文；点「显示密码」才单独解密取一次。
 * 密码只在本机当前 Windows 用户下能解，拷走 DB 也读不出来。
 */
export default function BrowserDataSettings() {
  const [tab, setTab] = useState<"history" | "credential">("history");
  const [hint, setHint] = useState("");
  const [busy, setBusy] = useState(false);

  const [history, setHistory] = useState<HistoryEntry[]>([]);
  const [search, setSearch] = useState("");

  const [creds, setCreds] = useState<CredentialEntry[]>([]);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [revealed, setRevealed] = useState<Record<number, string>>({});

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

  useEffect(() => {
    void loadHistory();
    void loadCreds();
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

  return (
    <>
      <h3>浏览器数据</h3>
      <p className="settings-hint">
        浏览历史自动记录（最多 3000 条，同网址 10 分钟内合并）；账号密码用 Windows DPAPI
        加密，只在本机当前用户下能解，DB 拷走也读不出来。
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
        </>
      )}
    </>
  );
}
