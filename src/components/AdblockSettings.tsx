import { useEffect, useState } from "react";
import { api } from "../api";
import type { BlockedEntry } from "../types";

/**
 * 设置 → 广告拦截。
 *
 * 画匣浏览器是 WebView2，**装不了 uBlock 这类扩展**；拦截在引擎的网络请求层做
 * （域名名单 + 联盟追踪参数），比扩展更底层，XHR/图片/脚本/弹窗跳转都能拦。
 */
export default function AdblockSettings() {
  const [enabled, setEnabled] = useState(
    () => window.localStorage.getItem("huaxia.d2.adblock") !== "0",
  );
  const [custom, setCustom] = useState("");
  const [hint, setHint] = useState("");
  const [busy, setBusy] = useState(false);
  const [blocked, setBlocked] = useState<BlockedEntry[]>([]);
  const [pickerOn, setPickerOn] = useState(
    () => window.localStorage.getItem("huaxia.d2.picker") !== "0",
  );

  const loadBlocked = async () => {
    try {
      setBlocked(await api.blockedList());
    } catch (error) {
      setHint(String(error));
    }
  };

  useEffect(() => {
    void api
      .adblockReadCustom()
      .then(setCustom)
      .catch(() => {});
    void loadBlocked();
  }, []);

  const removeBlocked = async (item: BlockedEntry) => {
    try {
      await api.blockedRemove(item.host, item.selector);
      await loadBlocked();
    } catch (error) {
      setHint(String(error));
    }
  };

  const clearBlocked = async () => {
    if (!window.confirm("恢复全部已屏蔽元素？")) return;
    try {
      await api.blockedClear();
      await loadBlocked();
    } catch (error) {
      setHint(String(error));
    }
  };

  const toggle = (next: boolean) => {
    setEnabled(next);
    window.localStorage.setItem("huaxia.d2.adblock", next ? "1" : "0");
  };

  const save = async () => {
    setBusy(true);
    setHint("");
    try {
      await api.adblockWriteCustom(custom);
      setHint("已保存，立即生效（新打开的页面 / 下一次刷新起）。");
    } catch (error) {
      setHint(`保存失败：${String(error)}`);
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <h3>广告拦截</h3>
      <p className="settings-hint">
        画匣浏览器基于 WebView2，<strong>装不了 uBlock 之类的扩展</strong>；这里的拦截在引擎的
        网络请求层完成，比扩展更底层——广告脚本、图片、XHR、弹窗跳转都能挡。
      </p>

      <label className="settings-row">
        <span>启用广告拦截</span>
        <input
          type="checkbox"
          checked={enabled}
          onChange={(event) => toggle(event.target.checked)}
        />
      </label>

      <label className="settings-row">
        <span>显示元素选择圆点（网页右下角 ⊘）</span>
        <input
          type="checkbox"
          checked={pickerOn}
          onChange={(event) => {
            setPickerOn(event.target.checked);
            window.localStorage.setItem(
              "huaxia.d2.picker",
              event.target.checked ? "1" : "0",
            );
          }}
        />
      </label>

      <h4>内置拦截</h4>
      <p className="settings-hint">
        内置名单覆盖主流广告网络（doubleclick / googlesyndication / taboola / outbrain /
        exoclick / popads 等）与成人站常见弹窗网络；
        <strong>另外会拦截带联盟/追踪参数的链接</strong>：`allianceid` / `utm_campaign` /
        `utm_source` / `utm_medium` / `trip_sub` / `gclid` / `fbclid` / `aff_id` 等——
        你贴的 trip.com 推广链接就是靠这层命中的，正常访问 trip.com 不受影响。
      </p>

      <h4>自定义名单</h4>
      <p className="settings-hint">
        一行一个域名（含子域自动匹配），`#` 开头为注释。保存后立即生效，无需重启。
      </p>
      <textarea
        className="settings-textarea"
        value={custom}
        onChange={(event) => setCustom(event.target.value)}
        placeholder={"# 例：\nads.example.com\ntracker.example.net"}
        spellCheck={false}
        rows={9}
      />
      <div className="browser-data-toolbar">
        <button
          type="button"
          className="btn btn-primary btn-compact"
          onClick={() => void save()}
          disabled={busy}
        >
          保存名单
        </button>
        {hint && <span className="settings-hint">{hint}</span>}
      </div>

      <h4>已屏蔽的元素</h4>
      <p className="settings-hint">
        浏览器页面右下角有个小圆点（⊘）：点它，再点网页上的元素，就能永久屏蔽。
        这里可以恢复（删除）屏蔽错的。
      </p>
      <div className="browser-data-list">
        {blocked.map((item) => (
          <div className="browser-data-row" key={`${item.host}|${item.selector}`}>
            <div className="browser-data-main">
              <span className="browser-data-title">{item.host}</span>
              <span className="browser-data-sub">{item.selector}</span>
            </div>
            <button
              type="button"
              className="browser-data-remove"
              onClick={() => void removeBlocked(item)}
              title="恢复（删除这条屏蔽）"
            >
              ×
            </button>
          </div>
        ))}
        {blocked.length === 0 && <p className="settings-hint">还没有屏蔽任何元素。</p>}
      </div>
      {blocked.length > 0 && (
        <div className="browser-data-toolbar">
          <button type="button" className="btn btn-quiet btn-compact" onClick={() => void clearBlocked()}>
            全部恢复
          </button>
        </div>
      )}

      <h4>还想更强？油猴脚本补 DOM 层</h4>
      <p className="settings-hint">
        引擎层拦不住“页面内嵌的原生广告位/反广告墙”时，可以用用户脚本补一层。到
        {' '}
        <a
          href="https://greasyfork.org/zh-CN/scripts?q=%E5%B9%BF%E5%91%8A%E6%8B%A6%E6%88%AA"
          target="_blank"
          rel="noreferrer"
        >
          Greasy Fork 搜「广告拦截」
        </a>
        {' '}
        挑一个装上（设置 → 用户脚本也支持直接贴 URL 安装）。注意油猴脚本只能改 DOM，
        挡不住网络请求，所以**优先靠上面的引擎层拦截**。
      </p>
    </>
  );
}
