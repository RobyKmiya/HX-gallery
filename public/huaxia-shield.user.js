// ==UserScript==
// @name         画匣盾 · 弹窗/广告跳转过滤
// @name:zh-CN   画匣盾 · 弹窗/广告跳转过滤
// @namespace    huaxia://shield
// @version      1.1.0
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
