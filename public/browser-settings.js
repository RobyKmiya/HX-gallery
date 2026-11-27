// 匣浏览器 · 设置面板
//
// 这是引擎在浏览器窗口里临时加的一条 webview（铺满窗口、页面自己画遮罩），
// 不是独立原生窗口。宿主与引擎只通过 `__xkReqQ` 队列通信，页面拿不到 IPC。
// 当前值由引擎在创建后 eval 写入 `window.__xkSettings`，可能晚于本脚本执行，
// 所以这里轮询等它出现再填充表单。
(function () {
  var $ = function (id) { return document.getElementById(id); };
  var filled = false;
  var saving = false;
  var waitTimer = null;

  function req(action, extra) {
    var payload = Object.assign({ action: action }, extra || {});
    (window.__xkReqQ = window.__xkReqQ || []).push(payload);
  }

  function msg(text, kind) {
    var el = $("msg");
    el.textContent = text;
    el.className = "msg" + (kind ? " " + kind : "");
  }

  function fill(s) {
    filled = true;
    $("home").value = s.home || "";
    $("newTab").value = s.newTab === "blank" ? "blank" : "home";
    $("searchUrl").value = s.searchUrl || "";
    var step = Math.round((Number(s.zoomStep) || 0.1) * 100);
    $("zoomStep").value = String(Math.min(50, Math.max(5, step)));
    $("zoomStepVal").textContent = $("zoomStep").value + "%";
    var width = Math.round((Number(s.tabsWidth) || 0.44) * 100);
    $("tabsWidth").value = String(Math.min(70, Math.max(20, width)));
    $("tabsWidthVal").textContent = $("tabsWidth").value + "%";
    $("shortcuts").checked = s.shortcuts !== false;
    $("showTitles").checked = s.showTitles !== false;
    $("adblock").checked = s.adblock !== false;
    $("picker").checked = s.picker !== false;
    $("saveAskDir").checked = s.saveAskDir === true;
    $("saveDir").value = s.saveDir || "";
  }

  function defaultSettings() {
    return {
      home: "https://e-hentai.org/",
      newTab: "home",
      searchUrl: "https://www.bing.com/search?q={q}",
      zoomStep: 0.1,
      shortcuts: true,
      showTitles: true,
      tabsWidth: 0.44,
      saveAskDir: true,
      saveDir: "",
      picker: true,
      adblock: true
    };
  }

  function waitSettings() {
    var s = window.__xkSettings;
    if (s && typeof s === "object") {
      fill(s);
      msg("设置保存在本机，改完点保存立刻生效。");
      return;
    }
    if (typeof waitTimer === "number" && Date.now() - waitTimer > 4000) {
      // 引擎没喂值（极少见）：退回默认值，至少让面板可用。
      fill(defaultSettings());
      msg("没读到当前设置，显示的是默认值。", "err");
      return;
    }
    if (typeof waitTimer !== "number") waitTimer = Date.now();
    setTimeout(waitSettings, 60);
  }

  $("zoomStep").addEventListener("input", function () {
    $("zoomStepVal").textContent = $("zoomStep").value + "%";
  });
  $("tabsWidth").addEventListener("input", function () {
    $("tabsWidthVal").textContent = $("tabsWidth").value + "%";
  });

  function close() {
    req("closeSettings");
  }

  $("close").addEventListener("click", close);
  $("cancel").addEventListener("click", close);
  // 1.1.97：设置是悬浮窗（webview 就卡片这么大），没有「点外部关闭」一说，
  // 关闭只走 × / 取消 / 保存 / Esc。
  document.addEventListener("keydown", function (e) {
    if (e.key === "Escape") { e.preventDefault(); close(); return; }
    var ctrl = e.ctrlKey || e.metaKey;
    if (ctrl && (e.key === "," || e.key === "s")) {
      e.preventDefault();
      if (e.key === "s") save();
      else close();
    }
  });

  function save() {
    if (saving) return;
    if (!filled) {
      msg("还没读到当前设置，稍等一下。", "err");
      return;
    }
    var home = $("home").value.trim();
    var search = $("searchUrl").value.trim();
    // 主页允许三种：普通 http(s) 网址、huaxia:newtab（⌂ 回画匣新页）、about:blank。
    if (!/^https?:\/\//i.test(home) && home !== "huaxia:newtab" && home !== "about:blank") {
      msg("主页要填完整网址（如 https://e-hentai.org/），或 huaxia:newtab / about:blank", "err");
      $("home").focus();
      return;
    }
    if (search.indexOf("{q}") < 0 || !/^https?:\/\//i.test(search)) {
      msg("搜索模板要有 {q} 占位符，并且是完整网址。", "err");
      $("searchUrl").focus();
      return;
    }
    saving = true;
    msg("正在保存…");
    req("config", {
      config: {
        home: home,
        newTab: $("newTab").value,
        searchUrl: search,
        zoomStep: Number($("zoomStep").value) / 100,
        shortcuts: $("shortcuts").checked,
        showTitles: $("showTitles").checked,
        tabsWidth: Number($("tabsWidth").value) / 100,
        saveAskDir: $("saveAskDir").checked,
        saveDir: $("saveDir").value.trim(),
        picker: $("picker").checked,
        adblock: $("adblock").checked
      }
    });
    // 引擎没回话就别永远转：4 秒后放开按钮。
    setTimeout(function () {
      if (saving) {
        saving = false;
        msg("保存超时，浏览器可能正忙，再点一次。", "err");
      }
    }, 4000);
  }
  $("save").addEventListener("click", save);

  // 引擎回执
  window.__xkSetSaved = function () {
    saving = false;
    msg("已保存，正在生效…", "ok");
    setTimeout(close, 420);
  };
  window.__xkSetError = function (text) {
    saving = false;
    msg(String(text || "保存失败"), "err");
  };

  waitSettings();
})();
