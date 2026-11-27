// 匣浏览器 · 工具栏逻辑
//
// 独立成外部文件而不是内联 <script>：正式包 CSP 是 script-src 'self'，
// 内联脚本会被整段拦掉（开发模式 devCsp 带 'unsafe-inline' 所以看不出问题）。
// 2026-11-23 管理员反馈「刷新和地址栏看不见」的根因就在这里：JS 没执行，
// render() 不跑、按钮事件没绑定。
//
// 2026-11-24：动作改「数组队列」。原来单槽 `window.__xkReq = {...}`，两下
// 连点（连点放大、切标签后立刻刷新）后一次会直接覆盖前一次，表现就是
// 「点了没反应」；现在 push 到 `__xkReqQ`，引擎一次取走整批。
(function () {
  // 内嵌模式（工作台统一标签条接管）：内层标签条收掉，只留地址栏/导航/缩放/设置。
  if (location.search.indexOf("embedded=1") >= 0) {
    var inner = document.getElementById("tabs");
    if (inner) inner.style.display = "none";
  }

  var state = null;
  var lastStateJson = "";
  var addrFocused = false;
  var toastTimer = null;
  var bootAt = Date.now();
  var stalled = false;

  var $ = function (id) { return document.getElementById(id); };

  function req(action, extra) {
    var payload = Object.assign({ action: action }, extra || {});
    (window.__xkReqQ = window.__xkReqQ || []).push(payload);
  }

  // 引擎执行动作失败/下载入库完成时调这个（Rust 侧 eval
  // `window.__xkToast("...", "ok"|"bad")`）。
  // 工具栏只有 38px 高，弹层会被裁掉，所以提示直接显示在地址栏里：
  // bad 闪红、ok 闪绿，锁标跟着换成 ⚠ / ✓。
  window.__xkToast = function (text, kind) {
    var wrap = $("addrWrap");
    if (!wrap) return;
    var ok = kind === "ok";
    wrap.classList.remove("bad", "ok");
    wrap.classList.add(ok ? "ok" : "bad");
    var addr = $("addr");
    var lock = $("lock");
    addr.title = String(text || (ok ? "完成" : "操作失败"));
    // 锁图标换成标记：地址栏有输入时不能顶掉用户正在敲的内容，
    // 但至少要让人看见「刚才那下有结果」。
    lock.textContent = ok ? "✓" : "⚠";
    lock.style.color = ok ? "var(--ok)" : "var(--danger)";
    if (!addrFocused) addr.value = String(text || (ok ? "完成" : "操作失败"));
    clearTimeout(toastTimer);
    toastTimer = setTimeout(function () {
      wrap.classList.remove("bad", "ok");
      lock.style.color = "";
      lock.textContent = /^https:/i.test((state && state.activeUrl) || "") ? "🔒" : "◉";
      addr.title = "输入网址或搜索词，回车前往（Ctrl+L 聚焦）";
      if (!addrFocused && state) addr.value = state.activeUrl || "";
      toastTimer = null;
    }, 2400);
  };

  function domainOf(url) {
    var m = /^[a-z]+:\/\/([^/]+)/i.exec(url || "");
    return m ? m[1].replace(/^www\./i, "") : (url || "空白页");
  }

  function setTabsWidth(pct) {
    var tabs = $("tabs");
    var value = Math.round((Number(pct) || 0.44) * 100);
    tabs.style.maxWidth = Math.max(20, Math.min(70, value)) + "vw";
  }

  function render() {
    if (!state) return;
    var settings = state.settings || {};
    setTabsWidth(settings.tabsWidth);

    // 标签条
    var tabs = $("tabs");
    tabs.textContent = "";
    (state.tabs || []).forEach(function (tab, index) {
      var chip = document.createElement("div");
      chip.className = "tab" + (tab.active ? " active" : "");
      chip.title = (tab.title ? tab.title + "\n" : "") + (tab.url || "");
      chip.draggable = true;

      if (tab.active && state.loading) {
        var sp = document.createElement("span");
        sp.className = "spinner";
        chip.appendChild(sp);
      }

      var label = document.createElement("span");
      label.className = "t";
      // 窄标签只显示域名：一整排「Loading…」比域名更难认。
      label.textContent = settings.showTitles === false
        ? domainOf(tab.url)
        : (tab.title || tab.url || "空白页");
      chip.appendChild(label);

      var x = document.createElement("span");
      x.className = "x";
      x.textContent = "×";
      x.title = "关闭标签";
      x.addEventListener("click", function (e) {
        e.stopPropagation();
        req("close", { tabId: tab.id });
      });
      chip.appendChild(x);

      chip.addEventListener("click", function () { req("switch", { tabId: tab.id }); });
      // 拖拽排序（类似 Edge）：拖到目标芯片上松手即换位
      chip.addEventListener("dragstart", function (e) {
        e.dataTransfer.effectAllowed = "move";
        e.dataTransfer.setData("text/plain", tab.id);
        chip.classList.add("dragging");
      });
      chip.addEventListener("dragend", function () { chip.classList.remove("dragging"); });
      chip.addEventListener("dragover", function (e) {
        e.preventDefault();
        e.dataTransfer.dropEffect = "move";
        chip.classList.add("drop-target");
      });
      chip.addEventListener("dragleave", function () { chip.classList.remove("drop-target"); });
      chip.addEventListener("drop", function (e) {
        e.preventDefault();
        e.stopPropagation();
        chip.classList.remove("drop-target");
        var dragged = e.dataTransfer.getData("text/plain") || tab.id;
        if (dragged && dragged !== tab.id) req("reorder", { tabId: dragged, index: index });
      });
      tabs.appendChild(chip);
    });

    // 地址栏：聚焦时不打断输入
    if (!addrFocused && $("addr") !== document.activeElement && !toastTimer) {
      $("addr").value = state.activeUrl || "";
      $("addr").title = state.activeUrl || "当前页面地址";
    }
    if (!addrFocused && !toastTimer) {
      $("lock").textContent = /^https:/i.test(state.activeUrl || "") ? "🔒" : "◉";
    }

    // 缩放：值来自引擎状态，切换标签自动跟着变（以前工具栏自己记 zoomPct，
    // 切到另一个标签后还显示上一个标签的百分比，看着像「缩放键坏了」）。
    var pct = Math.round((Number(state.zoom) || 1) * 100);
    $("zoom").textContent = pct + "%";
    $("zoom").title = "重置 100%（当前 " + pct + "%）";

    // 刷新按钮在加载中变成停止
    var reload = $("reload");
    if (state.loading) {
      reload.textContent = "✕";
      reload.title = "停止加载";
      reload.classList.add("busy");
    } else {
      reload.textContent = "⟳";
      reload.title = "刷新（Ctrl+R / F5）";
      reload.classList.remove("busy");
    }
  }

  // 兼容两种注入形态：Rust 侧 eval("window.__xkState = {...}") 给的是对象，
  // 历史上也有过字符串形态。原来只认 string，对象被直接跳过 → state 恒 null，
  // 地址栏和标签条永远空白（2026-11-23 修）。
  function tick() {
    var raw = window.__xkState;
    var normalized = null;
    if (raw && typeof raw === "object") normalized = JSON.stringify(raw);
    else if (typeof raw === "string" && raw) normalized = raw;
    if (normalized && normalized !== lastStateJson) {
      lastStateJson = normalized;
      try {
        state = typeof raw === "object" ? raw : JSON.parse(normalized);
      } catch (e) {
        state = null;
      }
      render();
    }
    // 内容页按 Ctrl+L：引擎把焦点交给工具栏并置这个标记。
    if (window.__xkFocusAddr) {
      window.__xkFocusAddr = false;
      focusAddr();
    }
    // 引擎心跳：工具栏是唯一始终可见的窗格，通道真的死掉时必须看得见。
    // （2026-11-24 的教训：状态表没初始化，整台引擎哑掉，界面却毫无反应，
    //  从外面看就是「按钮全坏了」。）
    if (!state && Date.now() - bootAt > 3000) {
      if (!stalled) {
        stalled = true;
        var wrap = $("addrWrap");
        if (wrap) wrap.classList.add("stalled");
        if (!addrFocused) {
          $("addr").value = "引擎无响应 · 点这里重连";
          $("addr").title = "工具栏与浏览器引擎的通道没通，点一下重新握手";
        }
      }
    } else if (state && stalled) {
      stalled = false;
      var wrap2 = $("addrWrap");
      if (wrap2) wrap2.classList.remove("stalled");
      if (!addrFocused) $("addr").value = state.activeUrl || "";
    }
    window.requestAnimationFrame(function () { setTimeout(tick, 120); });
  }
  setTimeout(tick, 120);
  // 页面就绪后握手，主动拉一次全量状态（避免首帧早于注入导致空白）。
  setTimeout(function () { req("__pull"); }, 200);

  function focusAddr() {
    var addr = $("addr");
    addrFocused = true;
    addr.value = (state && state.activeUrl) || "";
    addr.focus();
    setTimeout(function () { addr.select(); }, 0);
  }

  $("back").addEventListener("click", function () { req("back"); });
  $("forward").addEventListener("click", function () { req("forward"); });
  $("reload").addEventListener("click", function () {
    // 加载中再点是「停止」，符合浏览器直觉。
    req(state && state.loading ? "stop" : "reload");
  });
  $("home").addEventListener("click", function () { req("home"); });
  // 新建标签不传 url：由引擎按设置决定（主页 / 空白页）。
  // 以前传的是当前页地址，点一下「+」等于复制当前标签，看着像按钮坏了。
  $("newtab").addEventListener("click", function () { req("new"); });
  $("zoomIn").addEventListener("click", function () { req("zoomIn"); });
  $("zoomOut").addEventListener("click", function () { req("zoomOut"); });
  $("zoom").addEventListener("click", function () { req("zoomReset"); });
  $("settings").addEventListener("click", function () { req("openSettings"); });

  var addr = $("addr");
  addr.addEventListener("focus", function () {
    // 通道卡住时点地址栏 = 手动重握手（引擎 __pull 会无条件重推全量状态）。
    if (stalled) {
      stalled = false;
      bootAt = Date.now();
      var wrap = $("addrWrap");
      if (wrap) wrap.classList.remove("stalled");
      req("__pull");
    }
    addrFocused = true;
    addr.value = (state && state.activeUrl) || "";
    setTimeout(function () { addr.select(); }, 0);
  });
  addr.addEventListener("blur", function () {
    addrFocused = false;
    addr.value = (state && state.activeUrl) || "";
  });
  addr.addEventListener("keydown", function (e) {
    if (e.key === "Enter") {
      e.preventDefault();
      var v = addr.value.trim();
      if (!v) return;
      addrFocused = false;
      // 不再前端拼 https://：引擎会判断是网址还是搜索词（搜索走设置里的模板）。
      req("nav", { url: v });
      addr.blur();
    } else if (e.key === "Escape") {
      addrFocused = false;
      addr.value = (state && state.activeUrl) || "";
      addr.blur();
    }
  });

  // 焦点在工具栏上时的快捷键（焦点在网页里由内容页脚本接管）。
  document.addEventListener("keydown", function (e) {
    var ctrl = e.ctrlKey || e.metaKey;
    if (ctrl && !e.altKey) {
      var k = (e.key || "").toLowerCase();
      if (k === "t") { e.preventDefault(); req("new"); return; }
      if (k === "w") { e.preventDefault(); req("close"); return; }
      if (k === "r") { e.preventDefault(); req("reload"); return; }
      if (k === "l") { e.preventDefault(); focusAddr(); return; }
      if (k === "=" || k === "+") { e.preventDefault(); req("zoomIn"); return; }
      if (k === "-") { e.preventDefault(); req("zoomOut"); return; }
      if (k === "0") { e.preventDefault(); req("zoomReset"); return; }
      if (k === ",") { e.preventDefault(); req("openSettings"); return; }
      // Ctrl+Tab：循环切换标签（管理员 2026-11-28：单键循环，Ctrl+Shift 组合全砍）
      if (k === "tab") { e.preventDefault(); req("nextTab"); return; }
    }
    if (e.altKey && !e.ctrlKey && e.key === "ArrowLeft") { e.preventDefault(); req("back"); return; }
    if (e.altKey && !e.ctrlKey && e.key === "ArrowRight") { e.preventDefault(); req("forward"); return; }
    // Alt+E：元素屏蔽（工具栏这边转给页面执行，见引擎 pickElement）
    if (e.altKey && !e.ctrlKey && !e.shiftKey && (e.key || "").toLowerCase() === "e") {
      e.preventDefault(); req("pickElement"); return;
    }
    // Alt+H：回主页
    if (e.altKey && !e.ctrlKey && !e.shiftKey && (e.key || "").toLowerCase() === "h") {
      e.preventDefault(); req("home"); return;
    }
    // F12 / Ctrl+Shift+I：开发者工具
    if (e.key === "F12" || (e.ctrlKey && e.shiftKey && (e.key || "").toLowerCase() === "i")) {
      e.preventDefault(); req("devtools"); return;
    }
    if (e.key === "F5") { e.preventDefault(); req("reload"); }
  });

  render();
})();
