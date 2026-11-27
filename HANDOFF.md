# 画匣（HX-gallery）交接单

> 交接：2026-11-27 23:35 ｜ 交接人：workbuddy（DeepSeek-V4.1）
> 本轮（1.1.95）状态：已装机、待管理员复验，**交接时无未完成改动、无未答复的反馈**。
> 本文件是**自包含**的：新代理零上下文，从这份 + 第三节代码地图 + 第五节踩坑就能接手。
> 接手第一步：`node D:/DSH/agent-dispatch/board.mjs check --project huaxia` 核对登记，
> 再读 `projects/huaxia/state.json` 的 `note`（每轮验收反馈与修复要点都在里面，比这份更细）。

---

## 一、一句话现状

画匣已装 **1.1.95**（FileVersion 1.1.95 / exe mtime 见下）
源码 HEAD 见看板登记（功能 `69729d8`）
今天（11-27）从 **1.1.84 迭代到 1.1.95（12 版）**，全部已装机。

**18:01 那批「1.1.86 / 1.1.87 也一起修」已问清并修完**（管理员当场答复）：
- 1.1.86 里**只有「标签拖拽排序」没过**，其余视为通过；
- 1.1.87 里**「阅读浮动详情按钮」和「阅读设置齿轮入口」没过**，`;` `'` 书签与单击关详情已通过。
- 1.1.92 验收：**通过**的是 18comic 点「登錄」；**没过**的是「新标签打开 + 标签拖拽」「启动屏仍黑屏/无动效」「另存原图没有窗口、不知道存哪」。

**1.1.93 四项**：拖拽、另存通过；启动屏仍黑（CSP 拦掉壁纸缓存，见 18d）。
**1.1.94 四项**（壁纸黑屏 / 顶栏图标 / 面板互斥 / 新页卡顿）：**管理员 20:53 回复「1234 验收通过」**。
**1.1.95**：按管理员要求扩充浏览器快捷键（重点：元素屏蔽不再依赖页面圆点）。

## 二、项目与工具链

| 项 | 值 |
|---|---|
| 源码 | `D:\Qoderwork`（master） |
| 安装位置 | `C:\Users\Administrator\AppData\Local\画匣\archive-gallery.exe` |
| 技术栈 | Tauri 2.12 + Rust（`src-tauri/src/`）+ WebView2 + React19 + TS（`src/`） |
| 画廊库 | `F:\Hthings\BENZI`（**外接硬盘**，会休眠——今天一半的坑都和它有关） |
| 日常迭代发版 | `node scripts/release.mjs --fast`（约 4~5 分钟，补丁位 +1，自动提交 4 个版本文件并装机） |
| 默认压体积档 | `node scripts/release.mjs`（约 23 分钟，只在正式发布用） |
| ⚠️ 发版铁律 | release.mjs **只提交 4 个版本文件**——功能代码必须自己单独 commit，且**先 commit 功能再跑发版** |
| 看板核对 | `cd D:/DSH/agent-dispatch && node board.mjs check --project huaxia` |
| 推水位 | `node D:/DSH/agent-dispatch/hooks/dsh-hook.mjs mark-synced --repo D:/Qoderwork --sha <HEAD>` |
| 广播 | `node agentctl.mjs broadcast --project huaxia --as workbuddy --title "…" --text "…"`（每完成一批实质工作**主动**广播，不等催） |
| 收尾三件套 | `projects/huaxia/state.json`（version/head/exeMtime/note）+ `BOARD.md`（头部三行登记）+ `prompts/30-huaxia.md`（一句话摘要），然后 `board.mjs check` 必须「与登记状态一致」 |

**环境注意（今天实测）**
- `cargo check` / `cargo test` 在 **Bash 通道**可用（99 passed / 0 failed / 4 ignored）；个别命令会被沙箱安全策略拦（如 `reg.exe`、PowerShell 的 `Add-Type`/程序集反射加载），拦了不要硬重试，换等价手段。
- PowerShell 里**无法**加载 System.Drawing/WinForms 程序集 → **没有截图/GUI 自动化能力**，只能静态排查 + 真实数据实证（拆 CBZ、curl 探接口、node 校验注入脚本）。
- Python 可用（含 Pillow，`python -m pip install pillow` 已装）；注意 msys 的 `/tmp` ≠ Python 的 `TEMP`，给 Python 写文件用 `os.environ['TEMP']`。
- 出网 curl 走系统代理 `http://127.0.0.1:30808`（应用内 `netdl::proxy_arg()` 自动读注册表）；直连 18comic 图片域部分可达、主站必被 CF 拦。
- 发版脚本会「清除回收站（所有驱动器）」——管理员回收站有要恢复的东西时先提醒。
- 发版时 `export-tags.mjs` 需要**独占库文件**：画匣开着会报 readonly database，不影响安装包。
- 后台跑 release.mjs 时任务状态可能显示 `failed`，实际看日志尾部是否出现「新版 1.1.xx 已安装」——出现即成功。
- 本地 `cargo check` 可能改 `Cargo.lock`，记得一并提交。git 的 LF/CRLF 警告无害。

## 三、代码地图（新代理最容易迷路的模块）

| 模块 | 职责 |
|---|---|
| `netdl.rs` | 链接下载核心：三站解析、多任务管理器、curl 抓取、打包入库。公开：`cookie_for`/`proxy_arg`/`set_running_stage`/`task_is_canceled_public`/`download_and_package`（**8 个参数，含 source_url 写备注「源」**）/`JmDecode`/`curl_base` |
| `jm_decode.rs` | 18comic 图片竖条混淆还原。**段数只信公式（官方行为），不要加任何「画面自校验」**——见第五节第 5 条，这正是修了三轮的真凶。webp 读写回环有测试 |
| `jm_api.rs` | 18comic 移动 API（jmcomic 协议）：token=md5(ts+`185Hcomic3PAPP7R`)，base64→AES-256-ECB 解 JSON；域名 cdnhjk/cdngwc 系列按序试。**`/login` 在接口域名上全部 404**（实测），登录不走这里；`api_login` 现在是「网页表单 POST → 移动 API 兜底」，但**浏览器里的登录已改为页内 fetch** |
| `page_ops.rs` | 画廊单页增删：zip/cbz 重打包；删除页落 `.gallery/trash/pages/{id}/`；缓存键自带 mtime+size |
| `reader.rs` + `pagecache.rs` | 阅读器页读取 + LRU 字节缓存。缓存键带「mtime+size」，替换包后旧键自然失效 |
| `site_login.rs` | 登录态：回填按**标志 cookie** 判定（18comic=AVS / exhentai=ipb_member_id / nhentai=sessionid|access_token）；`credential_from_snapshot` 从 remember_id 解明文账密 |
| `browser.rs` | 内嵌浏览器引擎（**今天改动最多的文件**）：标签、油猴、GM 轮询、右键菜单（`CONTEXT_MENU_SCRIPT`）、18comic 登录桥（`JM_LOGIN_HOOK`）、自动填充（`AUTOFILL_SCRIPT` + 选择式 chip）、广告拦截、`new_tab`（内嵌模式改发 `huaxia://open-url-tab`）、`save_image_via_host`（另存，支持询问目的地） |
| `d2.rs` | 网页下载入库（`import_downloaded_meta` 把源 URL 写备注）；维护锁用 `wait_maintenance`（排队 3 分钟），**不要退回 try_lock** |
| `lib.rs` | 应用状态/命令注册/启动流程：**后台开库线程**（`library_opening` + `LibraryState.loading`）、`splash_finish`、壁纸锁 + **壁纸本地缓存**（`wallpaper-cache` + `wcache://` 协议）、启动屏 30s 兜底线程 |
| `container.rs` / `archive.rs` | zip/7z/rar 页枚举；`is_image_name`/`is_ignored_entry` 是页过滤白名单 |

前端关键文件：`usePointerDrag.ts`（指针拖拽排序，所有拖动都走它）、`App.tsx`（启动阶段/轮询开库/`huaxia://open-url-tab` 转发）、
`LibraryView.tsx`（主页/侧栏/详情/`runScan` 去重）、`WorkspaceTabs.tsx`（多标签 + **标签拖拽** + `openUrlTab`）、
`Reader.tsx`（阅读器 + 沉浸 body class）、`BootScreen.tsx`（启动屏，壁纸走缓存）、
`WallpaperLock.tsx`（锁屏，壁纸走缓存）、`BrowserDataSettings.tsx`（登录态/账号密码）、
`NetGalleryDialog.tsx`（下载页，完成回调触发全扫描）。公共页：`public/splash.html`（原生启动屏）、
`public/browser-settings.html/js`（浏览器 ⚙ 设置，含另存目的地）、`public/browser-toolbar.html/js`。

## 四、待管理员验收 / 待办（接手先问清）

### 1.1.95 待验收（快捷键）
- [ ] **Alt+E / Ctrl+Shift+E** 进「元素屏蔽」选择模式（再按退出；Esc 也退）。**重点**：故意找一个
      广告把右下角圆点盖住的页面，按 Alt+E 仍能进；右键菜单新增「屏蔽这个元素…（Alt+E）」也能进。
- [ ] **Alt+H** 回主页、**Alt+C** 复制当前页地址、**Ctrl+Shift+Tab** 上一个标签、
      **F12 / Ctrl+Shift+I** 开发者工具。
- [ ] 设置 → 浏览器数据 → 浏览器设置里的「快捷键」表已补全这些键。

### 1.1.94 已验（存档，管理员 20:53「1234 验收通过」）
- [x] **壁纸/启动屏不再黑**：锁屏应当看到壁纸（散图那张）；没壁纸时是深靛渐变，不是死黑。
      根因是 CSP 没放行 `wcache:`，缓存图一直被浏览器拦掉（详见第五节 3b）。
- [x] **阅读器顶栏**：双页时收藏是「左半实心心 / 右半实心心」，书签是「左半实心星 / 右半实心星」，
      没有「左/右」小字；点书签一次加、再点移除；详情是圆圈 i 图标（无文字）；齿轮是真正的齿轮。
- [x] **面板互斥**：图片管理 / 缩略图 / 书签列表 / 散图选择 / 阅读设置，点哪个只开哪个，不再叠一起。
- [x] **散图选择器**：列表式选散图区（名字 + 数量 + ✓），选定即记住，下次打开停在上次那个。
- [x] **新页不再卡**：切到「新页」滚动应顺滑（壁纸静止层 + 去掉模糊层 + 后台主页停止渲染）。

### 1.1.93 已验（存档）
- [x] 标签拖拽（标签中心 + 工作台标签条）——**管理员 19:51 验收通过**。
- [x] 另存原图弹保存框 + 真实路径 ——**管理员 19:51 验收通过**（「1.1.92右键原图保存通过」）。

- [ ] **标签拖拽排序（两处都要试）**：标签中心 `⋮⋮` 手柄按住拖动、工作台顶部标签条按住拖动，
      松手后顺序变化并记住；拖完不应顺手换激活标签。
- [ ] **阅读器顶栏**：齿轮图标在顶栏能看到（窄窗口也不消失），点了在阅读器内弹出「阅读设置」
      （6 个手势槽可改、快捷键速查、恢复默认手势）；顶栏「详情」是带文字的药丸按钮，`I` 键同样能开。
      **浮动详情按钮已按管理员要求撤掉，不要再加回来。**
- [ ] **启动动效**：双击桌面图标 → 原生小窗动效 ≥0.7s → 主窗加载屏（壁纸 + 三层条）走完 ≈0.95s 再进库，
      全程不黑屏、不白闪；外接硬盘休眠时加载屏持续有动效。
- [ ] **另存原图**：右键「另存原图」应弹系统保存框（默认已开「询问目的地」，可挂到图片夹任意目录），
      存完工具栏提示**真实路径**（不再是固定「图片\画匣下载」）。

### 1.1.92 已验（存档，勿重复当待办）
- [x] ③ 18comic 网页登录弹窗直接点「登錄」能登录（页内 fetch，绕 CF 指纹）——**管理员 18:xx 确认通过**。
- [ ] ③ 备注「源：」、新开网址白屏、下载「用时」、启动重复扫描、账密「填入/忽略」chip 等其余项：
      管理员本轮未点名，`;` `'` 书签、单击关详情、1.1.86 其余六项同此（视为通过与「未复验」之间，别主动改）。

### 更早遗留（低优先级，仍有效）
元素屏蔽记忆（重启失效，疑 SPA 不重注入）、nhentai tag 带数字剥除、18comic 误屏蔽恢复、屏蔽网址 UI 入口、断点续传、标签行为向 Edge 看齐、F12 开发者面板、更新画匣整合进设置 + GitHub 推流自动检查、镭射立体卡范式（预览已交，待批复手感）。

## 五、踩过的坑（每一条都真实翻过车，别重犯）

**18comic 图片还原（今天连修三轮，务必看完）**
1. 段数 md5 入参是**章节 photo_id**（不是专辑 id）+ **无后缀文件名**；单章本子两者恰好相等，所以「单章正常、多章花」。
2. `scramble_id` 页面取不到时退回 220980；`/album` 接口路径直接硬编码 220980。
3. **不要再加「画面自校验」**：2026-11-27 实测，真实漫画在 64px 缩略图上的相邻行差异打分完全失灵（还原后 32.3 vs 不动 26.8），把公式算对的段数否决成「不处理」，整本原样错位入库——管理员连报三次「还是错位」的真凶就是这层「聪明」的兜底。官方库从不校验。
4. 成品全是 **webp**：`image` crate 0.25 能读能写（lossless 编码，有 `decode_roundtrip_webp` 测试守着）；存盘失败会静默留混淆图，失败必须往 status 里写。
5. 实证方法：把库里的 CBZ 解出来用 Python/Pillow 复现 `permute` + 肉眼比对（ViewImage 小图预览），比读代码快。

**登录 / Cloudflare**
6. **`cf_clearance` 绑定客户端 TLS 指纹**：curl 不管带什么 cookie 都会被 CF 重新挑战（`fbricate` 实测：浏览器里过了盾也没用）。凡是要过盾的**交互**（登录）只能**在页面里同源 fetch**；抓页有 `fetch_html_smart`（curl 失败→已登录标签同源 fetch）。
7. 18comic 网页登录按钮「点了没反应」的根因：站点等 reCAPTCHA token，WebView2 里 gstatic 加载不出 → 站点 JS 直接 return。现方案：`JM_LOGIN_HOOK` 捕获阶段拦下登录键，页内 `fetch('/login')`（表单 5 字段），AVS 由 Set-Cookie 直接落 cookie jar → reload。**服务端不校验验证码**（官方 python 库同款）。
8. AVS 是**单设备会话**：任何设备新登录会把其他端挤掉线（页面提示「已在其他裝置登入」）。这是站点机制，别当 bug 修。
9. 回填判定必须按**标志 cookie**，不能用「有没有任意 cookie」。

**内嵌浏览器（browser.rs）**
10. **内嵌模式加 webview = 在同一个占位矩形上叠一层**，外壳工作台标签条看不见 → 「新标签打开」表现为源标签跳转、后退失效。`new_tab` 现在对 `embedded` 直接 `emit("huaxia://open-url-tab")`，前端 `App.tsx` 转发 `huaxia:open-url-tab` CustomEvent → `WorkspaceTabs.openUrlTab`。**别在嵌入模式里再 add_child**。
10b. **HTML5 拖拽在 Windows 上根本不会触发**：Tauri `dragDropEnabled` 默认 `true`
    （`tauri-utils-2.10.4/src/config.rs` 原文：「Disabling it is required to use HTML5 drag and drop
    on the frontend on Windows since we replace the drag drop handler of WebView2」）。它同时是
    `LibraryView`/`LoosePanel` 的 `onDragDropEvent`（文件拖入导入）赖以工作的东西，**不能一刀关掉**。
    1.1.86 标签排序 + 1.1.92 标签条两轮「拖不动」都是这个原因。现方案：`src/usePointerDrag.ts`
    用 pointer 事件自己排（`data-drag-id` + `elementFromPoint`），拖动元素必须 `pointer-events:none`，
    松手后 300ms 内屏蔽补发的 click。**以后凡是要拖，都用这个 hook，别再写 `draggable`。**
11. 右键菜单第三版才通：**Shadow DOM + document 捕获阶段 closeMenu 会在点项时先把宿主摘出 DOM**，真机 WebView2 与 headless 表现分歧（headless 全过、真机全哑）。现版本：普通 DOM + 内联样式 + 菜单项 **mousedown 直接执行** + 页内导航（back/fwd/reload/全选/复制不进队列）+ 入队动作给「✓ 已发送」回执。改这块务必 `node --check` 提取出的脚本。
12. 队列契约：`window.__xkReqQ`。`poll_tab_req`（250ms，激活页+后台轮转）原子认领 **new + save**；工具栏轮询器（200ms）drain **其余动作**。两个轮询器**必须按动作分家**，否则互相吞（曾导致右键菜单/快捷键时灵时不灵）。
13. `eval_json` 是 `eval_with_callback + recv_timeout`；重页面 eval 慢，超时会返回空串，别把它当失败。
14. 油猴 GM 跨域 XHR 的 curl 必须走 `proxy_arg()`（漏过 → 织机对 18comic 全 timeout）；垫片 `timeout` 只在宿主认领（running）后计时，排队不算。
15. 浏览器设置（`BrowserSettings`）加字段后，**`browser.rs` 里测试的 `BrowserSettings { … }` 字面量要补字段**（否则 E0063 只在你跑 `cargo test` 时炸）。
16. 另存/选文件夹用 `app.dialog().file().blocking_save_file()`：**必须离开主线程**（我们在线程里），`into_path()` 返回 `Result` 要显式 match。

**启动 / 库 / 外接硬盘**
17. WebView2 初始化走**主线程**：`setup` 里同步 `db::open_library`（外接硬盘唤醒）会连启动屏都起不来 → 黑屏无动效。现为后台线程开库 + `LibraryState.loading` + 前端 400ms 轮询（120s 超时兜底）。
18. 壁纸源在外接盘同样会卡开屏：`save_wallpaper_lock` 时把 file/loose 复制到 `AppData/wallpaper-cache`（md5 命名），`wcache://` 协议服务；`wallpaperDisplayUrls()` 缓存优先。**新增壁纸必须重新保存一次配置才生成缓存**。
18b. **启动屏交接别在 mount 就调 `splash_finish`**：那时 React 只是提交了 DOM，还没绘制，
    主窗亮起来是空帧 = 管理员说的「黑屏/无动效」。现在：双 rAF + 启动屏最短 700ms；
    库开得再快也让动效走完 950ms；主窗 `backgroundColor` 设 `#06070a` 防白闪。
18c. 壁纸锁来源是 **loose/archive** 时，启动屏第一次取缓存取不到（后端还得先开库查记录），
    会退回原地址；外接盘睡着就是一屏纯色。BootScreen 已在 1.6s 后补拉一次缓存。
18d. **新增自定义协议（`register_uri_scheme_protocol`）必须同步写进 CSP 的 `img-src`/`connect-src`**：
    1.1.92 做了 `wcache://` 壁纸缓存，前端也拿到了 `http://wcache.localhost/...` 的 URL，
    但 `tauri.conf.json` 的 CSP 只放行了 asset/gallery/loose —— 浏览器直接把图拦掉，
    **代码全对、功能全无**，表现就是锁屏/启动屏一片黑（管理员连报两轮「启动屏仍黑屏」）。
    排查捷径：`python` 读装机 exe 的字节，`data.find(b"wcache.localhost")` 看编译进去的 CSP 到底写了什么。
    另外 `wcache` 的缓存文件名是按「来源引用」算的（散图 id 没扩展名 → 一律 .jpg），
    实际多半是 webp，**MIME 要按文件头嗅探，不能信扩展名**。
18e. 隐藏的标签层别只 `visibility:hidden`：主页 LibraryView 有几百张卡片 + 封面 + 动效，
    只改可见性时它仍在后台布局绘制，切到「新页」就明显卡（管理员 2026-11-27）。
    用 `content-visibility:hidden` 一起停掉。**但浏览器标签层不能加**——里面是原生子 webview
    的占位矩形，靠真实布局量 rect，跳过布局会让引擎位置算错。
18f. 新页（NewTab）的壁纸不要挂在可滚动容器上：`background-image` + `overflow:auto` 会让滚动
    每帧重绘整张放大图。现在是 `.new-tab-bg` 静止层 + `.new-tab-scroll` 内容层，并去掉小部件上的
    `backdrop-filter`（整屏壁纸上叠 6 层模糊在 WebView2 上非常贵）。
18g. **油猴脚本与引擎注入脚本同一个 JS world**（都走 `initialization_script`），所以页面侧能力
    可以互相调用：`huaxia-shield.user.js` 暴露 `window.__hxPickerApi`，内容页的
    `CONTENT_SHORTCUTS_SCRIPT` 直接 `toggle()` —— 这样快捷键不经过宿主也不经过那颗圆点，
    广告把圆点盖住照样能进元素屏蔽（管理员 2026-11-27 的原话就是「盖住按钮我无法操作」）。
    页面里那颗 `position:fixed` 的小圆点要挂 `documentElement`（别挂 body：站点给 body 加
    transform/overflow 会把 fixed 定位带走），并挂个 MutationObserver 掉了补回来。
18h. `webview.open_devtools()` 只在 `debug_assertions` 或 tauri 的 `devtools` feature 下存在——
    只在 `cargo check`（dev）里过、release 编译才会炸。要用 F12 必须在 Cargo.toml 里开
    `tauri = { features = [..., "devtools"] }`，并跑一次 `cargo check --release` 确认。
19. 启动重复扫描冲突：`runScan` 用 `scanInFlightRef` 去重，「已有导入或扫描正在进行」静默跳过。

**计数 / 计时 / 文案**
20. 「下载用时 0.1s」是 `importer.rs` 的**入库耗时**冒充的；下载完成 stage 现在用任务线程 `Instant` 算真实耗时，入库消息改「入库耗时」。
21. 备注「源：」由 `download_and_package` 的 `source_url` 参数决定；jm API 路径的 referer 是接口域名（cdnhjk.net），**不能当源链接**。

**前端**
22. `workspace-tabs` 的 div 带**内联 style**（display:flex）——任何想隐藏它的 CSS 必须 `!important`（沉浸阅读隐藏顶栏踩过）。
23. 跨组件共享阅读偏好：Reader 用 `usePersistentFlag`、SettingsDialog 用 PrefToggle 直写 localStorage 同一键。
24. 详情页/阅读器改了画集 → `emit("d2://download", {ok:true})` 触发主页防抖刷新。
25. 下载页（NetGalleryDialog）是**嵌入标签**：切主页走标签切换，不触发 onClose；完成刷新靠 `onDone → runScan(true)`。
25b. **Reader 有两个实例**：`LibraryView`（主页）和 `GalleryTab`（工作台画廊标签）。
    任何「入口只在主页有」的 prop 都会在工作台标签里消失（齿轮就是这么丢的，1.1.87 报了两轮）。
    现在的规矩：**入口开在 Reader 自己身上**（齿轮 → 内置阅读设置面板、详情 → 顶栏 + I 键），
    不再靠外部传 `onOpenSettings` 之类的回调。改 Reader 顶栏务必两个实例都看。
25d. 阅读器内的浮层/侧栏（图片管理 / 缩略图 / 书签 / 散图选择 / 阅读设置）用一个 `panel` 状态做
    **互斥**，别再各开各的布尔：以前能同时开三四个，叠在一起看不清也点不准（管理员 2026-11-27）。
25e. 半实心图标（左页填左半 / 右页填右半）走 `Icons.tsx` 的 `HalfIcon`：同一 path 画一遍描边，
    再画一遍 `fill` 并用 `clipPath` 左右各裁一半；clipPath 的 id 固定（`hx-half-left/right`），
    多处重复定义无害。管理员明确要求「实心半边代表方向」，不要再用「左/右」小字角标。
25c. 顶栏右侧按钮会被挤没：`.reader-head-tools` 必须 `flex:0 0 auto`，标题/页名用 `max-width`
    省略，否则窄窗口下齿轮/关闭直接跑到窗口外。
26. 发版前**必跑**：`npx tsc --noEmit -p tsconfig.json` + `npm run build` + `cargo check` + `cargo test`（现状 99/0/4）。

## 六、版本史速查（1.1.85 → 1.1.95，细节见 BOARD 时间线与 state.json note）

| 版本 | 一句话 |
|---|---|
| 1.1.85 | UI 动效统一 A2–A15（stagger/面板/抽屉/扫光/弹跳/Toast/指示条/计数滚动/壁纸交叉淡化） |
| 1.1.86 | 去标签一键导图 + 标签拖拽排序 + 收藏夹滚动 + SERIES 折叠记忆 + 下载反向进度/烟花 + 刷新画廊按钮 + 进主界面扫新（**管理员验收通过**） |
| 1.1.87 | 阅读内浮动详情 + 单击关详情不翻页 + 工具栏齿轮 + `;` `'` 左右书签 |
| 1.1.88 | 18comic 多章拼接(aid=photo_id) + 整本累计进度 + gallery 详情链路 + 返回键按标签隔离 + API 登录绕验证码 |
| 1.1.89 | 启动加载屏 v2 + 壁纸锁禁拖动/设置内裁剪/无密码直进 + 织机 curl 走系统代理 + 弹窗治理 v3 |
| 1.1.90 | 原生启动屏 splash.html + 18comic 还原信公式（删自校验）+ 登录桥 + 沉浸隐藏顶栏 + 右键菜单一轮 + 关下载页刷库 |
| 1.1.91 | 后台开库防黑屏 + 沉浸顶栏 `!important` + 右键菜单二轮重造 + 网页表单登录（修 401）+ 备注源链接 + 下载完成全扫描 |
| 1.1.92 | 页内 fetch 登录（绕 CF 指纹）+ 壁纸本地缓存 + 另存目的地设置 + 嵌入真新签/标签拖拽 + 选择式填充 + 计时/扫描/白屏修复 |
| 1.1.95 | 浏览器快捷键扩充（Alt+E 元素屏蔽 / Alt+H 主页 / Alt+C 复制地址 / Ctrl+Shift+Tab 上一标签 / F12 开发者工具）+ 元素屏蔽不再依赖页面圆点（挂 documentElement、掉了自动补、遮罩提到最高层、右键菜单入口） |
| 1.1.94 | CSP 放行 wcache（壁纸缓存终于真能显示）+ wcache 按文件头嗅探 MIME + 顶栏图标统一（半实心心/星、详情去字、齿轮重画）+ 阅读器面板互斥 + 散图选择器重做 + 新页卡顿（静止壁纸层/去模糊/隐藏层停渲染） |
| 1.1.93 | 拖拽改指针事件（绕开 Tauri 吞 HTML5 拖拽）+ 阅读器顶栏收口（撤浮动详情/齿轮内置设置面板/顶栏防挤）+ 启动屏交接时机与最短动效 + 另存默认弹框并回报真实路径 |

## 七、新窗口开场

> 「读 `D:\Qoderwork\HANDOFF.md` 交接单，先跑 `board.mjs check --project huaxia` 核对，
> 然后收 1.1.95 那批（浏览器快捷键扩充 / 元素屏蔽不被广告挡住）的验收反馈，逐条修。」

> 上一版的口径（已办完，留档）：① 问清 1.1.86/1.1.87 哪几项没过（答：1.1.86 只有标签拖拽；1.1.87 浮动详情按钮 +
> 齿轮入口）② 收 1.1.92 反馈（没过：新标签/拖拽、启动黑屏、另存没窗口）。

管理员习惯：反馈常带截图（落 `.workbuddy\clipboard-images\` 或 blobs）；喜欢当天闭环、每轮发版可验；
架构决策自己拍板；对「修了两三次还没好」的事会直接质问原因——**先拿真实数据实证（拆包/探接口/node 校验脚本）再动手，别靠猜**。

## 八、凭据约定

交接文本、代码、日志、广播里**都不写 cookie / token / 密码**。登录态存 WebView2 profile 与 DPAPI 加密库；
`remember_id` 里的明文账密只用于登录提示（现在是「填入/忽略」chip），不外显、不落日志。
（已提醒管理员：曾贴进聊天的 18comic cookie 含明文密码，建议改密。）
