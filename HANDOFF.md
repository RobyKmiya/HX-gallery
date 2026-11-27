# 画匣（HX-gallery）交接单

> 交接：2026-11-27 18:40 ｜ 交接人：workbuddy（DeepSeek-V4.1）
> 本文件是**自包含**的：新代理零上下文，从这份 + 第三节代码地图 + 第五节踩坑就能接手。
> 接手第一步：`node D:/DSH/agent-dispatch/board.mjs check --project huaxia` 核对登记，
> 再读 `projects/huaxia/state.json` 的 `note`（每轮验收反馈与修复要点都在里面，比这份更细）。

---

## 一、一句话现状

画匣已装 **1.1.92**（FileVersion 1.1.92 / exe mtime **2026-11-27T17:47**），
源码 HEAD `186d55c`（功能 `5e98316` → 发版 `186d55c`），工作区干净，看板登记一致。
今天（11-27）从 **1.1.84 迭代到 1.1.92（9 版）**，全部已装机。
**1.1.86 管理员明示验收通过**；1.1.87 四项（阅读浮动详情/单击关详情/齿轮入口/`;` `'` 书签）后续轮次未被点名，按「其他验收均通过」理解应为通过，但管理员 18:01 说「1.1.86 和 1.1.87 也一起修了吧」后**立刻要求换窗口**——具体哪几项没过需要接手后先问清楚。
**1.1.92 八项改动全部待验收**（清单见第四节）。

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

前端关键文件：`App.tsx`（启动阶段/轮询开库/`huaxia://open-url-tab` 转发）、
`LibraryView.tsx`（主页/侧栏/详情/`runScan` 去重）、`WorkspaceTabs.tsx`（多标签 + **标签拖拽** + `openUrlTab`）、
`Reader.tsx`（阅读器 + 沉浸 body class）、`BootScreen.tsx`（启动屏，壁纸走缓存）、
`WallpaperLock.tsx`（锁屏，壁纸走缓存）、`BrowserDataSettings.tsx`（登录态/账号密码）、
`NetGalleryDialog.tsx`（下载页，完成回调触发全扫描）。公共页：`public/splash.html`（原生启动屏）、
`public/browser-settings.html/js`（浏览器 ⚙ 设置，含另存目的地）、`public/browser-toolbar.html/js`。

## 四、待管理员验收 / 待办（接手先问清）

### 1.1.92 待验收（八项，全部已装机）
- [ ] ① 启动屏加载动效可见、外接硬盘唤醒不卡界面；**壁纸缓存需先在设置→壁纸锁重新保存一次**才会有本地副本。
- [ ] ③ 18comic 网页登录弹窗直接点「登錄」能登录（页内 fetch，绕 CF 指纹）。
- [ ] ③ 备注「源：」是新下载本子的作品页 URL（旧本子不会自动改）。
- [ ] ③ 新开网址白屏是否改善（webview 深色底）。
- [ ] ③⑥ 下载完成「用时」是真实下载耗时，不再是 0.1s。
- [ ] ③ 启动不再弹「扫描失败：已有导入或扫描正在进行」。
- [ ] ⑤ 「在新标签打开链接」真的开新工作台网页标签（源标签不动、后退可用）；工作台标签可拖拽换位。
- [ ] ⑤ 另存原图：设置里开「询问目的地」后弹保存框，可设默认文件夹；自动填充改为「填入/忽略」chip。

### 管理员 18:01 的中断请求（原话）
> 「1.1.86 和 1.1.87 也一起修了吧」

- 1.1.86（标签拖拽排序/收藏夹滚动/SERIES 折叠记忆/下载反向进度+烟花/刷新画廊按钮/进主界面扫新）——**管理员 11-27 11:55 明示「1.1.86验收通过」**。
- 1.1.87 四项：阅读内浮动详情按钮、单击画面关详情不翻页、阅读手势设置齿轮入口、`;` `'` 左右页加书签（单页作用于当前页）。1.1.88 轮管理员说「1.1.87请修改」后列的是 1.1.88 五项，之后多轮「其他验收均通过」。
- **接手动作**：先问管理员「1.1.86/1.1.87 具体哪几项还不行、什么现象」，别盲改。重点怀疑 `;` `'` 书签与阅读手势设置入口。

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
11. 右键菜单第三版才通：**Shadow DOM + document 捕获阶段 closeMenu 会在点项时先把宿主摘出 DOM**，真机 WebView2 与 headless 表现分歧（headless 全过、真机全哑）。现版本：普通 DOM + 内联样式 + 菜单项 **mousedown 直接执行** + 页内导航（back/fwd/reload/全选/复制不进队列）+ 入队动作给「✓ 已发送」回执。改这块务必 `node --check` 提取出的脚本。
12. 队列契约：`window.__xkReqQ`。`poll_tab_req`（250ms，激活页+后台轮转）原子认领 **new + save**；工具栏轮询器（200ms）drain **其余动作**。两个轮询器**必须按动作分家**，否则互相吞（曾导致右键菜单/快捷键时灵时不灵）。
13. `eval_json` 是 `eval_with_callback + recv_timeout`；重页面 eval 慢，超时会返回空串，别把它当失败。
14. 油猴 GM 跨域 XHR 的 curl 必须走 `proxy_arg()`（漏过 → 织机对 18comic 全 timeout）；垫片 `timeout` 只在宿主认领（running）后计时，排队不算。
15. 浏览器设置（`BrowserSettings`）加字段后，**`browser.rs` 里测试的 `BrowserSettings { … }` 字面量要补字段**（否则 E0063 只在你跑 `cargo test` 时炸）。
16. 另存/选文件夹用 `app.dialog().file().blocking_save_file()`：**必须离开主线程**（我们在线程里），`into_path()` 返回 `Result` 要显式 match。

**启动 / 库 / 外接硬盘**
17. WebView2 初始化走**主线程**：`setup` 里同步 `db::open_library`（外接硬盘唤醒）会连启动屏都起不来 → 黑屏无动效。现为后台线程开库 + `LibraryState.loading` + 前端 400ms 轮询（120s 超时兜底）。
18. 壁纸源在外接盘同样会卡开屏：`save_wallpaper_lock` 时把 file/loose 复制到 `AppData/wallpaper-cache`（md5 命名），`wcache://` 协议服务；`wallpaperDisplayUrls()` 缓存优先。**新增壁纸必须重新保存一次配置才生成缓存**。
19. 启动重复扫描冲突：`runScan` 用 `scanInFlightRef` 去重，「已有导入或扫描正在进行」静默跳过。

**计数 / 计时 / 文案**
20. 「下载用时 0.1s」是 `importer.rs` 的**入库耗时**冒充的；下载完成 stage 现在用任务线程 `Instant` 算真实耗时，入库消息改「入库耗时」。
21. 备注「源：」由 `download_and_package` 的 `source_url` 参数决定；jm API 路径的 referer 是接口域名（cdnhjk.net），**不能当源链接**。

**前端**
22. `workspace-tabs` 的 div 带**内联 style**（display:flex）——任何想隐藏它的 CSS 必须 `!important`（沉浸阅读隐藏顶栏踩过）。
23. 跨组件共享阅读偏好：Reader 用 `usePersistentFlag`、SettingsDialog 用 PrefToggle 直写 localStorage 同一键。
24. 详情页/阅读器改了画集 → `emit("d2://download", {ok:true})` 触发主页防抖刷新。
25. 下载页（NetGalleryDialog）是**嵌入标签**：切主页走标签切换，不触发 onClose；完成刷新靠 `onDone → runScan(true)`。
26. 发版前**必跑**：`npx tsc --noEmit -p tsconfig.json` + `npm run build` + `cargo check` + `cargo test`（现状 99/0/4）。

## 六、版本史速查（1.1.85 → 1.1.92，细节见 BOARD 时间线与 state.json note）

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

## 七、新窗口开场

> 「读 `D:\Qoderwork\HANDOFF.md` 交接单，先跑 `board.mjs check --project huaxia` 核对，
> 然后：① 问管理员 1.1.86/1.1.87 到底哪几项没过（他 18:01 说『也一起修了吧』），
> ② 收 1.1.92 的验收反馈，逐条修。」

管理员习惯：反馈常带截图（落 `.workbuddy\clipboard-images\` 或 blobs）；喜欢当天闭环、每轮发版可验；
架构决策自己拍板；对「修了两三次还没好」的事会直接质问原因——**先拿真实数据实证（拆包/探接口/node 校验脚本）再动手，别靠猜**。

## 八、凭据约定

交接文本、代码、日志、广播里**都不写 cookie / token / 密码**。登录态存 WebView2 profile 与 DPAPI 加密库；
`remember_id` 里的明文账密只用于登录提示（现在是「填入/忽略」chip），不外显、不落日志。
（已提醒管理员：曾贴进聊天的 18comic cookie 含明文密码，建议改密。）
