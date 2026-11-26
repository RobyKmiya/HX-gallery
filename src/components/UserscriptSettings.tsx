import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api } from "../api";
import type { GmScriptMeta } from "../types";

/**
 * 设置 → 用户脚本（油猴）。
 *
 * 与 D2 面板里那份共用同一套后端命令；这里只是把「安装 / 启停 / 删除」搬到设置里，
 * 方便集中管理。脚本只在浏览器内容页执行（画匣库页面不跑）。
 */
export default function UserscriptSettings() {
  const [scripts, setScripts] = useState<GmScriptMeta[]>([]);
  const [url, setUrl] = useState("");
  const [hint, setHint] = useState("");
  const [busy, setBusy] = useState(false);

  const load = async () => {
    try {
      setScripts(await api.listGmScripts());
    } catch (error) {
      setHint(`读取脚本失败：${String(error)}`);
    }
  };

  useEffect(() => {
    void load();
  }, []);

  const importFile = async () => {
    const picked = await open({
      multiple: false,
      filters: [{ name: "油猴脚本", extensions: ["js"] }],
    });
    if (typeof picked !== "string") return;
    setBusy(true);
    setHint("");
    try {
      const meta = await api.addGmScriptFromPath(picked);
      setHint(`已安装：${meta.name}`);
      await load();
    } catch (error) {
      setHint(`安装失败：${String(error)}`);
    } finally {
      setBusy(false);
    }
  };

  const importUrl = async () => {
    const value = url.trim();
    if (!value) return;
    setBusy(true);
    setHint("");
    try {
      const meta = await api.addGmScriptFromUrl(value);
      setHint(`已安装：${meta.name}`);
      setUrl("");
      await load();
    } catch (error) {
      setHint(`安装失败：${String(error)}`);
    } finally {
      setBusy(false);
    }
  };

  const toggle = async (script: GmScriptMeta) => {
    try {
      await api.setGmScriptEnabled(script.id, !script.enabled);
      await load();
    } catch (error) {
      setHint(`切换失败：${String(error)}`);
    }
  };

  const remove = async (script: GmScriptMeta) => {
    if (!window.confirm(`删除脚本「${script.name}」？`)) return;
    try {
      await api.removeGmScript(script.id);
      await load();
    } catch (error) {
      setHint(`删除失败：${String(error)}`);
    }
  };

  return (
    <>
      <h3>用户脚本（油猴）</h3>
      <p className="settings-hint">
        支持 Chrome/Edge 生态的 .user.js；脚本在「画匣浏览器」的内容页执行，画匣自己的库页面不跑。
        与「链接下载」面板里那份脚本列表是同一套。
      </p>

      <div className="browser-data-toolbar">
        <button type="button" className="btn btn-quiet btn-compact" onClick={() => void importFile()} disabled={busy}>
          从本地安装 .user.js
        </button>
        <input
          value={url}
          onChange={(event) => setUrl(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter") void importUrl();
          }}
          placeholder="或粘贴脚本 URL"
          className="settings-inline-input"
        />
        <button type="button" className="btn btn-quiet btn-compact" onClick={() => void importUrl()} disabled={busy || !url.trim()}>
          从网址安装
        </button>
      </div>

      {hint && <p className="settings-hint">{hint}</p>}

      <div className="browser-data-list">
        {scripts.map((script) => (
          <div className="browser-data-row" key={script.id}>
            <div className="browser-data-main">
              <span className="browser-data-title">
                {script.name} {script.version ? `v${script.version}` : ""}
              </span>
              <span className="browser-data-sub">
                {script.description || (script.matches.length ? script.matches.join("，") : "（无描述）")}
              </span>
            </div>
            <button type="button" className="btn btn-quiet btn-compact" onClick={() => void toggle(script)}>
              {script.enabled ? "已启用" : "已停用"}
            </button>
            <button type="button" className="browser-data-remove" onClick={() => void remove(script)} title="删除">
              ×
            </button>
          </div>
        ))}
        {scripts.length === 0 && <p className="settings-hint">还没有脚本。可以装「漫画织机」等 .user.js。</p>}
      </div>
    </>
  );
}
