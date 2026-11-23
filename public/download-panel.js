/* 下载面板逻辑。
 *
 * 与外界的两种交互：
 * - 调用 Rust：window.__TAURI_INTERNALS__.invoke（本窗口已在 capabilities 里授权）
 * - 读进度：轮询 dl_status（不依赖事件系统，面板随时开关都不会漏状态）
 *
 * 注意：正式包 CSP 是 script-src 'self'，所以这段必须留在外部文件里，
 * 写进 <script> 内联块会被整段丢弃（工具栏踩过这个坑）。
 */
(function () {
  "use strict";

  var invoke = function (cmd, args) {
    return window.__TAURI_INTERNALS__.invoke(cmd, args || {});
  };

  function $(id) {
    return document.getElementById(id);
  }

  var state = {
    count: 0,
    scanning: false,
    running: false,
    finished: false,
    outputPath: "",
  };

  function setStatus(text, kind) {
    var node = $("status");
    node.textContent = text || "";
    node.className = "status" + (kind ? " " + kind : "");
  }

  function setBusy(running) {
    state.running = running;
    $("start").disabled = running || state.scanning || state.count === 0;
    $("cancel").disabled = !running;
    $("rescan").disabled = running || state.scanning;
  }

  function renderBar(done, total) {
    var pct = total > 0 ? Math.min(100, Math.round((done / total) * 100)) : 0;
    $("barfill").style.width = pct + "%";
  }

  async function scan() {
    if (state.scanning) return;
    state.scanning = true;
    state.count = 0;
    setBusy(false);
    $("title").textContent = "正在读取作品页…";
    $("meta").textContent = "会翻页收集整部图片，作品很长时需要几十秒";
    setStatus("扫描中：解析作品页 + 收集全部图片页…");
    try {
      var out = await invoke("dl_scan");
      state.count = out.count || 0;
      $("title").textContent = out.title || "（无标题）";
      $("meta").textContent = "共 " + state.count + " 张";
      $("site").textContent = siteName(out.site);
      $("from").value = "1";
      $("to").value = String(state.count);
      $("from").max = String(state.count);
      $("to").max = String(state.count);
      if (out.outDir) $("outdir").value = out.outDir;
      if (out.defaultConcurrency) $("conc").value = String(out.defaultConcurrency);
      renderBar(0, state.count);
      setStatus("扫描完成，可以开始下载", "ok");
      state.scanning = false;
      setBusy(false);
    } catch (error) {
      state.scanning = false;
      setBusy(false);
      $("title").textContent = "没读到作品页";
      $("meta").textContent = String(error);
      setStatus("扫描失败：" + String(error), "bad");
    }
  }

  function siteName(site) {
    return (
      {
        eh: "exhentai / e-hentai",
        nh: "nhentai",
        jm: "18comic",
        wn: "wnacg",
      }[site] || site || ""
    );
  }

  async function start() {
    if (state.running) return;
    var from = parseInt($("from").value, 10) || 1;
    var to = parseInt($("to").value, 10) || state.count;
    if (to < from) {
      setStatus("范围不对：结束张数要大于等于开始张数", "bad");
      return;
    }
    setBusy(true);
    state.finished = false;
    state.outputPath = "";
    $("open").classList.add("hidden");
    var payload = {
      from: from,
      to: to,
      concurrency: parseInt($("conc").value, 10) || 8,
      packageCbz: $("pkg").checked,
      importToLibrary: $("imp").checked,
      outDir: $("outdir").value,
    };
    try {
      await invoke("dl_start", payload);
      setStatus("已开始：" + (to - from + 1) + " 张，并发 " + payload.concurrency + " 路");
    } catch (error) {
      setBusy(false);
      setStatus("启动失败：" + String(error), "bad");
    }
  }

  async function poll() {
    try {
      var status = await invoke("dl_status");
      if (status.total > 0) renderBar(status.done, status.total);
      if (status.running) {
        setBusy(true);
        var text = status.stage || "下载中…";
        if (status.failed > 0) text += "（失败 " + status.failed + " 张）";
        setStatus(text);
      } else if (status.finished) {
        setBusy(false);
        state.finished = true;
        if (status.error) {
          setStatus("失败：" + status.error, "bad");
          $("open").classList.add("hidden");
        } else {
          state.outputPath = status.path || "";
          setStatus("完成，已存到：" + status.path, "ok");
          $("open").classList.remove("hidden");
        }
      } else {
        setBusy(false);
      }
    } catch (error) {
      /* 面板刚打开、Rust 还没就绪时忽略一次 */
    }
  }

  $("start").addEventListener("click", function () {
    void start();
  });
  $("cancel").addEventListener("click", function () {
    invoke("dl_cancel").catch(function () {});
    setStatus("已请求取消…");
  });
  $("rescan").addEventListener("click", function () {
    void scan();
  });
  $("all").addEventListener("click", function () {
    $("from").value = "1";
    $("to").value = String(state.count || 1);
  });
  $("pick").addEventListener("click", function () {
    invoke("dl_pick_folder")
      .then(function (path) {
        if (path) $("outdir").value = path;
      })
      .catch(function (error) {
        setStatus("选目录失败：" + String(error), "bad");
      });
  });
  $("openOut").addEventListener("click", function () {
    invoke("dl_open_folder", { path: $("outdir").value }).catch(function (error) {
      setStatus(String(error), "bad");
    });
  });
  $("open").addEventListener("click", function () {
    var target = state.outputPath || $("outdir").value;
    invoke("dl_open_folder", { path: target }).catch(function (error) {
      setStatus(String(error), "bad");
    });
  });

  setBusy(false);
  window.requestAnimationFrame(function () {
    void scan();
  });
  setInterval(function () {
    void poll();
  }, 400);
})();
