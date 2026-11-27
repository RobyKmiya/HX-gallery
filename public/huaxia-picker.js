// 画匣元素选择器（引擎恒注入，不依赖用户脚本）
//
// 为什么从「画匣盾」用户脚本里拆出来：元素屏蔽（Alt+E / Ctrl+Shift+E / 右键
// 「屏蔽这个元素」）是核心能力，必须每页可用。以前它活在 huaxia-shield.user.js
// 里——用户脚本一旦被停用/某页没注入，`__hxPickerApi` 就不存在，快捷键直接
// 「没启用」（管理员 2026-11-28）。现在改成引擎 initialization_script 恒注入，
// 与广告拦截解耦。圆点显隐由引擎注入的 `window.__hxPicker`（boolean）决定，
// ⚙面板/主设置改完会调 `window.__hxSetPicker` 实时切换。
(function () {
  "use strict";
  if (window.__hxPickerApi) return;

  var pickerEnabled = true;
  try {
    if (typeof window.__hxPicker === "boolean") pickerEnabled = window.__hxPicker;
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
    // 全屏透明遮罩：选中模式下由它吃掉所有鼠标事件，<a> 收不到点击就不会跳转。
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
      // Alt+E 快捷键照样能进元素屏蔽。
      if (!pickerEnabled) return;
      // 挂 documentElement：站点给 body 加 transform/overflow 时，fixed 圆点是
      // 相对 body 定位的，会被裁掉或跟着滚走。
      document.documentElement.appendChild(dot);
    }

    mount();
    // 站点脚本会整段重写 body、或反广告脚本把陌生节点删掉 → 圆点消失，补回来。
    try {
      new MutationObserver(function () {
        if (!document.getElementById("__hxPicker")) mount();
      }).observe(document.documentElement, { childList: true });
    } catch (e) {}

    // 引擎侧快捷键（Alt+E / Ctrl+Shift+E / 右键「屏蔽这个元素」）都走这里。
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

    // 设置里的「显示元素选择圆点」实时开关：只影响圆点显隐。
    window.__hxSetPicker = function (on) {
      pickerEnabled = !!on;
      if (!pickerEnabled && !picking) {
        try { dot.remove(); } catch (e) {}
        return;
      }
      if (pickerEnabled && !picking) mount();
    };
  }

  // 恒执行：__hxPickerApi 必须一直存在，快捷键才永远可用。
  setupPicker();
})();
