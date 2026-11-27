import { useEffect, useState } from "react";
import { api } from "../api";

/**
 * 设置 → 匣浏览器。
 *
 * 1.1.96 整合：这里展示的是**完整的浏览器设置**（浏览器 ⚙ 面板里有的这里全有），
 * 读写同一份 settings.json（browser_settings_get / browser_settings_set）。
 * 以前浏览器设置只能开浏览器后点 ⚙ 才能改；现在主页设置直达，改完
 * 「重开浏览窗口生效」（引擎在建窗时读 settings.json）。
 */

type BrowserPrefs = {
  home: string;
  newTab: string;
  searchUrl: string;
  zoomStep: number;
  shortcuts: boolean;
  showTitles: boolean;
  tabsWidth: number;
  saveAskDir: boolean;
  saveDir: string;
  picker: boolean;
  adblock: boolean;
};

export default function BrowserPrefsSettings() {
  const [prefs, setPrefs] = useState<BrowserPrefs | null>(null);
  const [hint, setHint] = useState("");

  useEffect(() => {
    void api
      .browserSettingsGet()
      .then((raw) => setPrefs(raw as unknown as BrowserPrefs))
      .catch((error) => setHint(`读取浏览器设置失败：${String(error)}`));
  }, []);

  const patch = (next: Partial<BrowserPrefs>) => {
    setPrefs((prev) => (prev ? { ...prev, ...next } : prev));
    void api
      .browserSettingsSet(next)
      .then(() => setHint("已保存（重开浏览窗口生效）"))
      .catch((error) => setHint(`保存失败：${String(error)}`));
  };

  if (!prefs) {
    return (
      <>
        <h3>匣浏览器</h3>
        <p className="settings-hint">{hint || "正在读取浏览器设置…"}</p>
      </>
    );
  }

  return (
    <>
      <h3>匣浏览器</h3>
      <p className="settings-hint">
        内嵌浏览器的全部设置都在这里（与浏览窗口里点 ⚙ 打开的面板同一份数据）。
        改完 <strong>重开浏览窗口生效</strong>；浏览窗口 ⚙ 面板保存的项立即生效。
      </p>

      <label className="settings-row">
        <span>主页</span>
      </label>
      <input
        type="text"
        value={prefs.home}
        onChange={(event) => setPrefs({ ...prefs, home: event.target.value })}
        onBlur={() => patch({ home: prefs.home.trim() })}
        placeholder="https://e-hentai.org/"
        spellCheck={false}
        className="settings-text-input"
      />
      <p className="settings-hint">⌂ 按钮跳这里；新建标签选「主页」时也用它。</p>

      <label className="settings-row">
        <span>新建标签打开</span>
        <select
          value={prefs.newTab === "blank" ? "blank" : "home"}
          onChange={(event) => patch({ newTab: event.target.value })}
        >
          <option value="home">主页</option>
          <option value="blank">空白页</option>
        </select>
      </label>

      <label className="settings-row">
        <span>地址栏搜索模板</span>
      </label>
      <input
        type="text"
        value={prefs.searchUrl}
        onChange={(event) => setPrefs({ ...prefs, searchUrl: event.target.value })}
        onBlur={() => patch({ searchUrl: prefs.searchUrl.trim() })}
        placeholder="https://www.bing.com/search?q={q}"
        spellCheck={false}
        className="settings-text-input"
      />
      <p className="settings-hint">
        输入的内容不像网址时走这里；<code>{"{q}"}</code> 是搜索词占位符。
      </p>

      <label className="settings-row">
        <span>缩放步进（点 − / + 每次变化多少）</span>
        <select
          value={Math.round(prefs.zoomStep * 100)}
          onChange={(event) => patch({ zoomStep: Number(event.target.value) / 100 })}
        >
          {[5, 10, 15, 20, 25, 30, 40, 50].map((step) => (
            <option key={step} value={step}>
              {step}%
            </option>
          ))}
        </select>
      </label>

      <label className="settings-row">
        <span>标签栏最大宽度占比</span>
        <select
          value={Math.round(prefs.tabsWidth * 100)}
          onChange={(event) => patch({ tabsWidth: Number(event.target.value) / 100 })}
        >
          {[20, 30, 44, 55, 70].map((step) => (
            <option key={step} value={step}>
              {step}%
            </option>
          ))}
        </select>
      </label>

      <label className="settings-row">
        <span>启用内容页快捷键（Ctrl+T / W / R / L、Alt+←/→、Ctrl+Tab、Ctrl+,、F12）</span>
        <input
          type="checkbox"
          checked={prefs.shortcuts !== false}
          onChange={(event) => patch({ shortcuts: event.target.checked })}
        />
      </label>

      <label className="settings-row">
        <span>标签显示网页标题（关掉只显示域名）</span>
        <input
          type="checkbox"
          checked={prefs.showTitles !== false}
          onChange={(event) => patch({ showTitles: event.target.checked })}
        />
      </label>

      <label className="settings-row">
        <span>启用广告拦截（引擎网络层；自定义名单在「广告拦截」页维护）</span>
        <input
          type="checkbox"
          checked={prefs.adblock !== false}
          onChange={(event) => patch({ adblock: event.target.checked })}
        />
      </label>

      <label className="settings-row">
        <span>显示元素选择圆点（⊘；只管圆点，Alt+E / Ctrl+Shift+E 随时可用）</span>
        <input
          type="checkbox"
          checked={prefs.picker !== false}
          onChange={(event) => patch({ picker: event.target.checked })}
        />
      </label>

      <label className="settings-row">
        <span>另存原图时询问目的地（右键「打开原图并另存」弹保存框）</span>
        <input
          type="checkbox"
          checked={prefs.saveAskDir === true}
          onChange={(event) => patch({ saveAskDir: event.target.checked })}
        />
      </label>

      <label className="settings-row">
        <span>另存默认文件夹</span>
      </label>
      <input
        type="text"
        value={prefs.saveDir}
        onChange={(event) => setPrefs({ ...prefs, saveDir: event.target.value })}
        onBlur={() => patch({ saveDir: prefs.saveDir.trim() })}
        placeholder="留空 = 图片\画匣下载"
        spellCheck={false}
        className="settings-text-input"
      />
      <p className="settings-hint">不开「询问目的地」时用这里；文件夹不存在会自动创建。</p>

      {hint && <p className="settings-hint">{hint}</p>}
    </>
  );
}
