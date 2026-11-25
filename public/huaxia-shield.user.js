// ==UserScript==
// @name         画匣盾 · 弹窗/广告跳转过滤
// @name:zh-CN   画匣盾 · 弹窗/广告跳转过滤
// @namespace    huaxia://shield
// @version      1.0.0
// @description  画匣内置：挡广告弹窗、静默 alert/confirm、清明显广告位；轻量无依赖
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
  // 与引擎名单保持同一套关键词，避免两层各拦一半。
  var AD_MARKS = [
    "allianceid=", "utm_campaign=", "utm_source=", "utm_medium=", "trip_sub",
    "gclid=", "fbclid=", "aff_id=", "click_id=",
    "doubleclick", "googlesyndication", "googleadservices",
    "exoclick", "popads", "popcash", "propellerads", "trafficjunky",
    "juicyads", "adsterra", "hilltopads", "onclickmega", "clickadu",
    "popunder", "popmyads", "adcash", "adspyglass", "tsyndicate",
  ];

  function isAdUrl(url) {
    if (!url) return false;
    var s = String(url).toLowerCase();
    return AD_MARKS.some(function (mark) {
      return s.indexOf(mark) >= 0;
    });
  }

  // 1) 广告弹窗：命中广告特征直接吞掉；同源弹窗放行（站点登录/看图器等正常功能）。
  var nativeOpen = window.open;
  window.open = function (url) {
    try {
      if (isAdUrl(url)) return null;
    } catch (e) {}
    return nativeOpen.apply(window, arguments);
  };

  // 2) 静默 alert / confirm / prompt：广告弹层最常用的骚扰方式。
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

  function start() {
    clean(document);
    var observer = new MutationObserver(function (mutations) {
      for (var i = 0; i < mutations.length; i++) {
        var added = mutations[i].addedNodes;
        if (added && added.length) {
          clean(mutations[i].target.parentNode || document);
        }
      }
    });
    observer.observe(document.documentElement || document, {
      childList: true,
      subtree: true,
    });
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", start, { once: true });
  } else {
    start();
  }
})();
