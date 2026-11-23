// 匣浏览器 · 工具栏逻辑
//
// 独立成外部文件而不是内联 <script>：正式包 CSP 是 script-src 'self'，
// 内联脚本会被整段拦掉（开发模式 devCsp 带 'unsafe-inline' 所以看不出问题）。
// 2026-11-23 管理员反馈「刷新和地址栏看不见」的根因就在这里：JS 没执行，
// render() 不跑、按钮事件没绑定。
(function () {
  var state = null;
  var lastStateJson = "";
  var addrFocused = false;
  var zoomPct = 100;
  var grabBusy = false;

  var $ = function (id) { return document.getElementById(id); };

  function req(action, extra) {
    var payload = Object.assign({ action: action }, extra || {});
    window.__xkReq = payload;
  }

  function render() {
    if (!state) return;
    // 标签条
    var tabs = $("tabs");
    tabs.textContent = "";
    (state.tabs || []).forEach(function (tab, index) {
      var chip = document.createElement("div");
      chip.className = "tab" + (tab.active ? " active" : "");
      chip.title = tab.url;
      chip.draggable = true;
      var label = document.createElement("span");
      label.className = "t";
      label.textContent = tab.title || tab.url || "空白页";
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
      chip.addEventListener("click", function () {
        req("switch", { tabId: tab.id });
      });
      // 拖拽排序（类似 Edge）：拖到目标芯片上松手即换位
      chip.addEventListener("dragstart", function (e) {
        e.dataTransfer.effectAllowed = "move";
        e.dataTransfer.setData("text/plain", tab.id);
        chip.classList.add("dragging");
      });
      chip.addEventListener("dragend", function () {
        chip.classList.remove("dragging");
      });
      chip.addEventListener("dragover", function (e) {
        e.preventDefault();
        e.dataTransfer.dropEffect = "move";
        chip.classList.add("drop-target");
      });
      chip.addEventListener("dragleave", function () {
        chip.classList.remove("drop-target");
      });
      chip.addEventListener("drop", function (e) {
        e.preventDefault();
        e.stopPropagation();
        chip.classList.remove("drop-target");
        var dragged = e.dataTransfer.getData("text/plain") || tab.id;
        if (dragged && dragged !== tab.id) {
          req("reorder", { tabId: dragged, index: index });
        }
      });
      tabs.appendChild(chip);
    });
    // 地址栏：聚焦时不打断输入
    if (!addrFocused && $("addr") !== document.activeElement) {
      $("addr").value = state.activeUrl || "";
      // 完整网址放进 title，鼠标悬停可看全，方便核对查询参数细节。
      $("addr").title = state.activeUrl || "当前页面地址";
      $("lock").textContent = /^https:/i.test(state.activeUrl || "") ? "🔒" : "◉";
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
    // 抓取中的忙碌态由 Rust 侧写回 __xkGrab，避免按钮被重复点。
    if (window.__xkGrab === "busy" && !grabBusy) {
      grabBusy = true;
      var g = $("grab");
      if (g) { g.textContent = "…"; g.title = "正在抓取本页…"; }
    } else if (window.__xkGrab !== "busy" && grabBusy) {
      grabBusy = false;
      var g2 = $("grab");
      if (g2) { g2.textContent = "⬇"; g2.title = "抓取本页整部漫画入库"; }
    }
    window.requestAnimationFrame(function () { setTimeout(tick, 120); });
  }
  setTimeout(tick, 120);
  // 页面就绪后握手，主动拉一次全量状态（避免首帧早于注入导致空白）。
  setTimeout(function () { req("__pull"); }, 200);

  $("back").addEventListener("click", function () { req("back"); });
  $("forward").addEventListener("click", function () { req("forward"); });
  $("reload").addEventListener("click", function () { req("reload"); });
  $("home").addEventListener("click", function () { req("home"); });
  // 抓取本页整部：原来在 D2 面板里，管理员要求挪到浏览器工具栏上。
  $("grab").addEventListener("click", function () {
    if (grabBusy) return;
    req("grab");
  });
  $("newtab").addEventListener("click", function () {
    req("new", { url: (state && state.activeUrl) || "https://e-hentai.org/" });
  });
  $("zoomIn").addEventListener("click", function () {
    zoomPct = Math.min(400, zoomPct + 10);
    $("zoom").textContent = zoomPct + "%";
    req("zoomIn");
  });
  $("zoomOut").addEventListener("click", function () {
    zoomPct = Math.max(25, zoomPct - 10);
    $("zoom").textContent = zoomPct + "%";
    req("zoomOut");
  });
  $("zoom").addEventListener("click", function () {
    zoomPct = 100;
    $("zoom").textContent = "100%";
    req("zoomReset");
  });

  var addr = $("addr");
  addr.addEventListener("focus", function () {
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
      if (!/^https?:\/\//i.test(v)) v = "https://" + v;
      addrFocused = false;
      req("nav", { url: v });
      addr.blur();
    } else if (e.key === "Escape") {
      addrFocused = false;
      addr.value = (state && state.activeUrl) || "";
      addr.blur();
    }
  });

  render();
})();
