// ==UserScript==
// @name         画匣盾 · 弹窗/广告跳转过滤
// @name:zh-CN   画匣盾 · 弹窗/广告跳转过滤
// @namespace    huaxia://shield
// @version      1.3.0
// @description  画匣内置：挡广告弹窗/广告弹层、静默 alert/confirm、清明显广告位；轻量无依赖
// @author       画匣
// @match        https://*/*
// @match        http://*/*
// @run-at       document-start
// @grant        GM_getValue
// @grant        GM_setValue
// @grant        GM_addStyle
// ==/UserScript==
(function () {
  "use strict";
  if (window.__huaxiaShield) return;
  window.__huaxiaShield = true;

  var host = (location.hostname || "").toLowerCase();
  // 画匣自身页面 / 本地调试：不碰，免得把正常弹窗也吞了。
  var BYPASS = ["tauri.localhost", "localhost", "127.0.0.1"];
  var bypassed = BYPASS.some(function (h) {
    return host === h || host.indexOf(h + ".") === 0;
  });

  var enabled = true;
  try {
    if (typeof GM_getValue === "function") {
      enabled = GM_getValue("huaxia.shield.enabled", "1") !== "0";
    }
  } catch (e) {}
  if (!enabled || bypassed) return;

  // 广告/联盟特征：引擎层管网络请求，这里管「页面内的跳转/弹窗」。
  var AD_MARKS = [
    "allianceid=", "utm_campaign=", "utm_source=", "utm_medium=", "trip_sub",
    "gclid=", "fbclid=", "aff_id=", "click_id=",
    "doubleclick", "googlesyndication", "googleadservices",
    "exoclick", "popads", "popcash", "propellerads", "trafficjunky",
    "juicyads", "adsterra", "hilltopads", "onclickmega", "clickadu",
    "popunder", "popmyads", "adcash", "adspyglass", "tsyndicate",
    // 2026-11-26 补：ourdreamland.ai 那类推广跳转的参数组合。
    "clickid=", "tracker=", "affiliate=", "ref=tr", "ref=ad", "ref=aff",
  ];

  function isAdUrl(url) {
    if (!url) return false;
    var s = String(url).toLowerCase();
    return AD_MARKS.some(function (mark) {
      return s.indexOf(mark) >= 0;
    });
  }

  // 1) 广告弹窗：命中广告特征直接吞掉。
  var nativeOpen = window.open;
  window.open = function (url) {
    try {
      if (isAdUrl(url)) return null;
    } catch (e) {}
    return nativeOpen.apply(window, arguments);
  };

  // 2) 静默 alert / confirm / prompt。
  window.alert = function () {};
  window.confirm = function () {
    return false;
  };
  window.prompt = function () {
    return null;
  };

  // 3) 清广告位：只认高置信选择器，宁可少删也不误删正文。
  var AD_SELECTORS = [
    'iframe[src*="doubleclick"]',
    'iframe[src*="googlesyndication"]',
    'iframe[src*="adservice"]',
    'iframe[src*="exoclick"]',
    'iframe[src*="popads"]',
    'iframe[src*="propellerads"]',
    'iframe[src*="trafficjunky"]',
    'iframe[src*="adsterra"]',
    'iframe[src*="juicyads"]',
    'iframe[src*="hilltopads"]',
    "ins.adsbygoogle",
    ".adsbygoogle",
    '[id^="ad-slot-"]',
    '[class^="ad-slot-"]',
    "[data-ad-slot]",
  ];

  function clean(root) {
    for (var i = 0; i < AD_SELECTORS.length; i++) {
      var nodes;
      try {
        nodes = (root || document).querySelectorAll(AD_SELECTORS[i]);
      } catch (e) {
        continue;
      }
      for (var j = nodes.length - 1; j >= 0; j--) {
        var node = nodes[j];
        if (node && node.parentNode) node.parentNode.removeChild(node);
      }
    }
  }

  // 4) 广告弹层（管理员 2026-11-26：VPN 那种同站 DOM 弹窗，window.open 拦不到）。
  //    判定三件套：position:fixed/sticky + z-index 够高 + 文案含明确广告词。
  //    正常登录/设置对话框不含这些词，不会被误杀。
  var AD_TEXT = [
    "不限流量", "免费试用", "蓝莓vpn", "vpn性价比", "立即下载", "一键下载",
    "看片神器", "免费领取", "限时免费", "点击下载", "view more",
    "性价比王", "翻墙首选", "破解版", "专属优惠", "新人礼包",
  ];

  function checkLayer(el) {
    if (!el || el.nodeType !== 1) return;
    var style;
    try {
      style = window.getComputedStyle(el);
    } catch (e) {
      return;
    }
    if (style.position !== "fixed" && style.position !== "sticky") return;
    var z = parseInt(style.zIndex || "0", 10);
    if (!(z >= 100)) return;
    var rect = el.getBoundingClientRect();
    if (rect.width < 240 || rect.height < 120) return;
    var text = (el.innerText || "").toLowerCase().slice(0, 800);
    if (!text) return;
    for (var i = 0; i < AD_TEXT.length; i++) {
      if (text.indexOf(AD_TEXT[i]) >= 0) {
        el.remove();
        return;
      }
    }
  }

  function scanLayers(root) {
    var nodes;
    try {
      nodes = (root || document).querySelectorAll("div, section, aside, dialog");
    } catch (e) {
      return;
    }
    for (var i = nodes.length - 1; i >= 0; i--) checkLayer(nodes[i]);
  }

  // 4b) 兜底：没被文本命中时，自动点广告弹层自己的关闭按钮（只点 fixed + 高层级的）。
  function clickAdClose() {
    var btns = document.querySelectorAll(
      '[class*="close" i], [aria-label*="close" i], [aria-label*="关闭"], [title*="关闭"]',
    );
    for (var i = 0; i < btns.length; i++) {
      var btn = btns[i];
      var layer = btn.closest("div, section, aside");
      if (!layer) continue;
      var style;
      try {
        style = window.getComputedStyle(layer);
      } catch (e) {
        continue;
      }
      if (style.position !== "fixed") continue;
      if (parseInt(style.zIndex || "0", 10) < 100) continue;
      try {
        btn.click();
      } catch (e) {}
    }
  }

  // 5) 元素选择器（管理员 2026-11-26）：右下角小圆点 → 点网页元素 → 永久屏蔽。
  var pickerEnabled = true;
  try {
    // 引擎注入的开关优先（设置→广告拦截 里的「显示元素选择圆点」）；
    // 没有注入时退回脚本自己的 GM 存值。
    if (typeof window.__hxPicker === "boolean") {
      pickerEnabled = window.__hxPicker;
    } else if (typeof GM_getValue === "function") {
      pickerEnabled = GM_getValue("huaxia.shield.picker", "1") !== "0";
    }
  } catch (e) {}

  function cssEscape(value) {
    if (window.CSS && CSS.escape) return CSS.escape(value);
    return String(value).replace(/[^\w-]/g, "\\$&");
  }

  function selectorOf(el) {
    if (!el || el.nodeType !== 1) return "";
    if (el.id) return "#" + cssEscape(el.id);
    var parts = [];
    var node = el;
    while (node && node.nodeType === 1 && parts.length < 5) {
      if (node.id) {
        parts.unshift("#" + cssEscape(node.id));
        break;
      }
      var name = node.tagName.toLowerCase();
      if (name === "html" || name === "body") {
        parts.unshift(name);
        break;
      }
      var cls = "";
      if (node.className && typeof node.className === "string") {
        var list = node.className.trim().split(/\s+/).filter(Boolean).slice(0, 2);
        cls = list
          .map(function (c) {
            return "." + cssEscape(c);
          })
          .join("");
      }
      var nth = 1;
      var sib = node;
      while ((sib = sib.previousElementSibling)) {
        if (sib.tagName === node.tagName) nth++;
      }
      parts.unshift(name + cls + (nth > 1 ? ":nth-of-type(" + nth + ")" : ""));
      node = node.parentElement;
    }
    return parts.join(" > ");
  }

  function setupPicker() {
    var dot = document.createElement("div");
    dot.id = "__hxPicker";
    dot.title = "画匣元素选择器：点击后点网页元素永久屏蔽（Esc 退出）";
    dot.style.cssText = [
      "position:fixed", "right:18px", "bottom:18px", "width:26px", "height:26px",
      "border-radius:50%", "background:rgba(124,147,255,.92)",
      "box-shadow:0 2px 8px rgba(0,0,0,.4)", "z-index:2147483647",
      "cursor:pointer", "display:flex", "align-items:center",
      "justify-content:center", "color:#fff", "font-size:14px",
      "user-select:none", "transition:transform .15s",
    ].join(";");
    dot.textContent = "⊘";

    var picking = false;
    var current = null;
    var overlay = null;
    // 全屏透明遮罩：选中模式下由它吃掉所有鼠标事件，<a> 收不到点击就不会跳转
    // （1.1.61 的坑：直接给 document 挂 click 兜不住链接导航）。
    var shield = null;

    function pickAt(x, y) {
      var els = document.elementsFromPoint(x, y);
      for (var i = 0; i < els.length; i++) {
        var el = els[i];
        if (!el || el === shield || el === overlay || el === dot) continue;
        if (el === document.documentElement || el === document.body) continue;
        return el;
      }
      return null;
    }

    function highlight(el) {
      if (!el || el === current) return;
      current = el;
      if (!overlay) {
        overlay = document.createElement("div");
        overlay.id = "__hxPickerOverlay";
        overlay.style.cssText = [
          "position:fixed", "pointer-events:none", "z-index:2147483646",
          "border:2px solid #ff5c5c", "background:rgba(255,92,92,.12)",
          "border-radius:3px", "display:none",
        ].join(";");
        document.documentElement.appendChild(overlay);
      }
      var rect = el.getBoundingClientRect();
      overlay.style.display = "block";
      overlay.style.left = rect.left + "px";
      overlay.style.top = rect.top + "px";
      overlay.style.width = rect.width + "px";
      overlay.style.height = rect.height + "px";
    }

    function onMove(event) {
      if (!picking) return;
      var el = pickAt(event.clientX, event.clientY);
      if (el) highlight(el);
    }

    function onMouseDown(event) {
      if (!picking) return;
      if (event.target === dot) return;
      event.preventDefault();
      event.stopPropagation();
    }

    function onClick(event) {
      if (!picking) return;
      if (event.target === dot) return;
      event.preventDefault();
      event.stopPropagation();
      var el = pickAt(event.clientX, event.clientY) || current;
      if (!el) return;
      var selector = selectorOf(el);
      if (!selector) return;
      el.style.display = "none";
      el.setAttribute("data-hx-blocked", "1");
      try {
        (window.__xkReqQ = window.__xkReqQ || []).push({
          action: "blockElement",
          selector: selector,
          host: location.hostname,
        });
      } catch (e) {}
      stopPicking();
    }

    function onKey(event) {
      if (event.key === "Escape") {
        event.preventDefault();
        stopPicking();
      }
    }

    function startPicking() {
      if (picking) return;
      picking = true;
      current = null;
      if (!shield) {
        shield = document.createElement("div");
        shield.id = "__hxPickerShield";
        shield.style.cssText =
          "position:fixed;inset:0;z-index:2147483647;background:transparent;cursor:crosshair;";
        shield.addEventListener("mousemove", onMove, true);
        shield.addEventListener("mousedown", onMouseDown, true);
        shield.addEventListener("click", onClick, true);
        document.documentElement.appendChild(shield);
      }
      // 圆点跟着升到遮罩里：遮罩盖住整页（含各种高 z-index 广告）时，
      // 圆点仍是可点的退出入口，不用去找 Esc。
      try {
        if (dot.parentNode !== shield) shield.appendChild(dot);
        dot.style.zIndex = "2147483647";
      } catch (e) {}
      if (overlay) overlay.style.display = "none";
      document.addEventListener("keydown", onKey, true);
      dot.style.transform = "scale(1.15)";
      dot.title = "元素选择中：点元素屏蔽，Esc 退出";
    }

    function stopPicking() {
      if (!picking) return;
      picking = false;
      current = null;
      if (shield) {
        shield.remove();
        shield = null;
      }
      if (overlay) overlay.style.display = "none";
      try {
        // 圆点只在开关开着时回挂到页面上；关着就别再出现了。
        if (pickerEnabled && dot.parentNode !== document.documentElement) {
          document.documentElement.appendChild(dot);
        }
      } catch (e) {}
      dot.style.transform = "";
      dot.title = "画匣元素选择器：点击后点网页元素永久屏蔽（Esc 退出 / Alt+E 切换）";
      document.removeEventListener("keydown", onKey, true);
    }

    dot.addEventListener("click", function (event) {
      event.preventDefault();
      event.stopPropagation();
      if (picking) stopPicking();
      else startPicking();
    });

    function mount() {
      if (document.getElementById("__hxPicker")) return;
      // 圆点开关只管圆点显隐：关掉时 API（__hxPickerApi）照常注册，
      // Alt+E 快捷键照样能进元素屏蔽（管理员 2026-11-28：快捷键不该被开关挡）。
      if (!pickerEnabled) return;
      // 挂 documentElement：站点给 body 加 transform/overflow 时，fixed 圆点是
      // 相对 body 定位的，会被裁掉或跟着滚走（管理员 2026-11-27：广告盖住按钮）。
      document.documentElement.appendChild(dot);
    }

    mount();
    // 站点脚本会整段重写 body、或反广告脚本把陌生节点删掉 → 圆点消失。
    // 只在 documentElement 的直接子节点上守（便宜），掉了就补回来。
    try {
      new MutationObserver(function () {
        if (!document.getElementById("__hxPicker")) mount();
      }).observe(document.documentElement, { childList: true });
    } catch (e) {}

    // 引擎侧快捷键（Alt+E / Ctrl+Shift+E）走这里：不依赖圆点，页面上任何
    // 遮挡物挡住按钮都能进/出屏蔽模式。
    window.__hxPickerApi = {
      toggle: function () {
        if (picking) stopPicking();
        else startPicking();
        return picking;
      },
      start: startPicking,
      stop: stopPicking,
      isOn: function () {
        return picking;
      },
    };

    // 设置里的「显示元素选择圆点」实时开关（⚙面板保存 / 主设置修改都会调）：
    // 只影响圆点显隐，不动正在进行的屏蔽模式。
    window.__hxSetPicker = function (on) {
      pickerEnabled = !!on;
      if (!pickerEnabled && !picking) {
        try { dot.remove(); } catch (e) {}
        return;
      }
      if (pickerEnabled && !picking) mount();
    };
  }

  // setupPicker **恒执行**：__hxPickerApi 必须一直存在，Alt+E 才永远可用；
  // 圆点本身由 pickerEnabled / __hxSetPicker 控制显隐。
  setupPicker();

  var sweepTimer = null;
  function sweep() {
    if (sweepTimer) return;
    sweepTimer = window.setTimeout(function () {
      sweepTimer = null;
      scanLayers(document);
      clickAdClose();
    }, 300);
  }

  function start() {
    clean(document);
    sweep();
    var observer = new MutationObserver(function (mutations) {
      for (var i = 0; i < mutations.length; i++) {
        var added = mutations[i].addedNodes;
        if (added && added.length) {
          clean(mutations[i].target.parentNode || document);
          for (var j = 0; j < added.length; j++) {
            var node = added[j];
            if (node.nodeType === 1) {
              checkLayer(node);
              if (node.querySelectorAll) scanLayers(node);
            }
          }
          sweep();
        }
      }
    });
    observer.observe(document.documentElement || document, {
      childList: true,
      subtree: true,
    });
    // 有些广告延迟 1~3 秒才插进来，补两轮定时扫。
    window.setTimeout(sweep, 1200);
    window.setTimeout(sweep, 3000);
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", start, { once: true });
  } else {
    start();
  }
})();
