# 画匣（HX-gallery）交接单

> 生成：2026-11-26 15:10 ｜ 交接人：workbuddy
> 新窗口接手：先读这份，再跑 `node D:/DSH/agent-dispatch/board.mjs check --project huaxia` 核对状态。

## 一、一句话现状

画匣已装 **1.1.75**（FileVersion 1.1.75 / mtime 2026-11-26T19:42:04），
源码 HEAD `c170862`，工作区干净，看板登记一致（`check` 全绿）。浏览器广告拦截、元素选择器、
网络画集下载（exhentai / nhentai / 18comic 单章）都已上；**7 项功能待办**见第四节，
其中「18comic 图片是否仍错位」要等管理员验收后才能决定要不要做切片还原。

## 二、项目与工具链

| 项 | 值 |
|---|---|
| 源码 | `D:\Qoderwork`（master） |
| 安装位置 | `C:\Users\Administrator\AppData\Local\画匣\archive-gallery.exe` |
| 技术栈 | Tauri 2 + Rust + WebView2 + React（前端 `src/`） |
| 发版 | 三处版本号（`package.json` / `src-tauri/Cargo.toml` / `src-tauri/tauri.conf.json`）→ commit → push → GitHub Actions 云编译（8~12 分钟） |
| 取产物 | 下载 run 的 artifact zip → 解压 `archive-gallery.exe` → 覆盖安装位置 → 启动验证 |
| 广播 | `node D:/DSH/agent-dispatch/agentctl.mjs broadcast --as workbuddy --task huaxia-tabs-145 --status <in_progress|done> --title "…" --text "…"` |
| 收尾 | 同步 `.handoff-tabs.md` + `projects/huaxia/BOARD.md` + `state.json` + `prompts/30-huaxia.md` → `board.mjs check` → `hooks/dsh-hook.mjs mark-synced --repo D:/Qoderwork` |

**环境注意**
- `cargo test` 在 Bash 通道会被 SIGTERM；本地验证用 PowerShell 跑 `cargo check`（写日志再看 `^error`）。
- 本仓在 D 盘，Bash 正常可用；E 盘（影匣）才需要 PowerShell 通道。
- 发布前**先本地 `cargo check`**，能省一次 10 分钟的云编译失败。
- 本地 `cargo check` 会改 `Cargo.lock`，记得一并提交，否则 `check` 报脏。

## 三、本会话已完成（1.1.57 → 1.1.69）

| 版本 | 内容 |
|---|---|
| 1.1.57 | 广告拦截加**联盟追踪参数**（`allianceid/utm_*/trip_sub/gclid/fbclid/aff_id/click_id/clickid/tracker/ref=tr|ad|aff`）；自定义名单热生效 |
| 1.1.58 | 内置「画匣盾」油猴脚本（`public/huaxia-shield.user.js`），启动自动写入脚本仓库 |
| 1.1.59 | 画匣盾识别**同站 DOM 广告弹层**（fixed + z-index + 广告文案） |
| 1.1.60 | **元素选择器**：右下角 ⊘ 圆点 → 点元素永久屏蔽；`blocked_elements.rs` + `blocked.json`；设置页可恢复 |
| 1.1.61 | 元素选择圆点开关（设置→广告拦截；`localStorage huaxia.d2.picker` → `d2_embed_open` 的 `picker` → `window.__hxPicker`） |
| 1.1.62 | 修「点元素会跳转」：全屏透明遮罩 `#__hxPickerShield` 接管鼠标事件 |
| 1.1.63 | 网络画集**多链接队列**（一行一个，串行逐本下载） |
| 1.1.64 | 地址栏 **SPA 回填**（hook pushState/replaceState/popstate）；补 clickid/tracker |
| 1.1.66 | 网络画集 cookie 改问**浏览器 webview**；nhentai API 适配 |
| 1.1.67 | exhentai tag 锚点放宽；nhentai 接口被拦时回退解析作品页 |
| 1.1.68 | **坏代理端口根因**（见第五节）；Cloudflare 站改走浏览器；18comic 单章 |
| 1.1.69 | 18comic 图片**按 DOM 顺序**取（修错位）；18comic tag；图片 content-type 校验 |
| 1.1.71 | **账号记忆**：登录态 Cookie 快照落盘（DPAPI）+ 下载时兜底 + 粘贴 Cookie 自动存；注入 cookie 改 180 天持久；登录框按账号簿自动填充；设置→浏览器数据加「登录态」分区 |
| 1.1.72 | **四连修**：18comic 竖条混淆还原（`jm_decode.rs`）/ nhentai 改 v2 API / 标签面板始终挂载保状态 / 内置浏览器顶栏首帧摆位 |
| 1.1.73 | 修 1.1.72 引入的主页空白回归（面板容器 home 时也要 hidden）；18comic tag 三路解析（data-type span / meta keywords / 旧 href） |
| 1.1.74 | **下载管理器**：多任务并行/单项启停/防卡死（cookie 缓存绕开 WebView2 + curl 可 kill + 全局闸门）；UI 卡片化美化 |
| 1.1.81 | 下载入库把源 URL 写进画集备注，详情页可点开（新网页标签）；修 Reader 关闭时重开原生标题栏导致的「双框」；壁纸锁壁纸可拖拽平移/滚轮缩放，加载失败可见 |
| 1.1.80 | 18comic 移动 API 源数据路线（`jm_api.rs`，jmcomic 协议 AES 解密，整本/多章一次拿全）+ 登录框从快照 `remember_id` 自动填充账密 + exhentai tag 改织机整页 /tag/ 解析；拍板不接外接浏览器 |
| 1.1.79 | 「Edge 打开」试水口：`open_in_edge`（独立 profile + --app），登录类站点交真 Edge 管 cookie/CF/扩展；二期 CDP 互通 cookie/下载待立项 |
| 1.1.78 | 登录态快照可见化：设置→登录态 显示各站已存 cookie 名，缺标志 cookie 直接给补救指引（`site_login_detail` 只回名不回值） |
| 1.1.77 | 18comic 深度适配：登录回填按**标志 cookie** 判定（18comic=AVS / exhentai=ipb_member_id / nhentai=sessionid）；导入维护锁改排队等；下载完成自动刷新列表；下载图与织机 XHR 自动重试；exhentai tag 改 /tag/ 链接解析 |
| 1.1.76 | 18comic 段数改**画面自校验**（公式段数不可信时自动纠正）；浏览器性能：GM 轮询改轮转、队列先查长度、广告匹配零分配、UA 探测后台化；前进/后退到底给提示；登录态同步节流；登录态加快捷粘贴块 |
| 1.1.75 | **下载全挂/登录态丢失/广告劫持三连修**：图片判定改文件头魔数（CT 常为空）、curl 复用 WebView2 真实 UA（cf_clearance 绑 UA）、抓页改已登录标签同源 fetch（不再 navigate 劫持标签）、`site_login` 自动快照+回填、`site_login_import` 粘贴 Cookie 入库、广告域名导航级拦截、侧栏移除画匣浏览器 |

## 四、待办（按优先级，管理员 2026-11-26 原话）

1. **壁纸锁拖不动**：壁纸锁界面顶部留一条可拖动区域（只给拖动权限，不影响解锁）。
2. **元素屏蔽没记忆**：管理员反馈重启后屏蔽失效。排查方向：
   - `blocked.json` 是否写入（`__xkReqQ` 的 `blockElement` 是否到达 `handle_toolbar_action`）；
   - 注入只挂在 `on_page_load Finished`，**SPA 路由不会重注入** → 需要像地址栏那样监听 pushState 后补注入。
3. **nhentai tag 带数字**：nhentai 的 tag 常跟作品数，入库时剥掉，只留 tag 名。
4. **18comic 被误屏蔽、看不了画廊**：怀疑元素选择器误伤画廊容器；加保护 + 支持批量恢复。
5. ~~**18comic 记不住账号密码**：每次要重登，账号簿要支持保存/自动填充。~~ → **1.1.71 已做**（登录态快照 + 自动填充，待人工验收）。注意：站点登录若用「会话 cookie」（没写 Expires），仍可能重启失效——这类站点需在浏览器登录后再到 设置→浏览器数据→登录态 手工保存一次。
6. **屏蔽元素管理 UI 不清、与广告屏蔽位置重合**：拆独立分区，入口明确。
7. **屏蔽网址功能未上线**：域名黑名单后端已有（`adblock.txt` + 设置页自定义名单），缺「按网址屏蔽」的 UI 入口。
8. **18comic album（整本）合并**：现在只支持 `/photo/` 单章；`/album/` 需章节列表 → 逐章抓图 → 合并一个 CBZ。
9. ~~**18comic 打散切片**（条件性）：若 1.1.69 后仍错位，说明该站图片是打散切片，需按 DOM 位置/CSS 还原。~~ → **1.1.72 已做**（`jm_decode.rs`，竖条混淆还原，待验收）。
10. 其余：下载页断点续传、rememberWindowPosition 开关、D2Panel 迁标签、阅读器内嵌浏览器多标签、下载面板多链接。

## 五、踩过的坑（别重犯）

1. **坏代理端口**：`HTTPS_PROXY=127.0.0.1:50714` 是失效旧代理，系统代理是 `30808`。
   老实现环境变量优先 → 所有 curl 打死端口（「网络问题很大 / 全部超时」）。
   `netdl.rs::proxy_arg` 已改**系统代理优先**，**别改回去**。
2. **Cloudflare**：nhentai / 18comic 对 curl 返回 `403 + Just a moment`。
   `fetch_html_smart`（curl 失败 → `browser::load_html` 用 WebView2 导航并读 `outerHTML`）。
   **会占用当前浏览器标签**。
3. **WebView2 装不了扩展**：uBlock/AdGuard 都不行。拦截靠引擎层（`browser.rs`
   `on_web_resource_request`）+ 内置油猴脚本。
4. **油猴脚本是烘焙的**：`huaxia-shield.user.js` 在浏览窗口创建时注入，**改完必须重启/重开浏览窗口**。
5. **`ensure_builtin_scripts` 必须「内容不同就覆盖」**：1.1.58 写成「存在就跳过」，新规则永远装不上。
6. **写死 HTML 锚点会漏**：`parse_tags` 原写死 `<div id="taglist">`，带 style/class 就整块找不到。
7. **按属性分组提取会乱序**：18comic 图片原来分 `data-original`/`src` 两轮抓，页序全乱；必须按 `<img>` 顺序。
8. **图片要校验 Content-Type**：图床超时返回 200 的 HTML 错误页，存成 `.jpg` 就是「Error: timeout」占位图。

## 六、待管理员验收

- [ ] exhentai `4357384` 的 tag 是否出来（1.1.67）。
- [ ] nhentai `699872` 是否走浏览器抓取成功（1.1.68）；失败时是否明确报「下载到的不是图片」（1.1.69）。
- [ ] 18comic `/photo/1492995` 页序是否正确、tag 是否出现（1.1.69）。
- [ ] 元素屏蔽重启后是否仍生效（**已知问题，待办 2**）。
- [ ] 壁纸锁拖动（**未做，待办 1**）。
- [ ] 账号记忆（1.1.71）：浏览器登录 → 设置→浏览器数据→登录态 保存 → 关窗/重启 → 网络下载是否免登录；登录页是否自动填账号密码。
- [ ] 四连修（1.1.72）：18comic 下载页序是否正确（还原后）、nhentai `699959` 能否下载、标签离开返回是否保留搜索词/子页、新标签顶栏是否对齐不错位。
- [ ] 1.1.73：主页画集列表是否恢复、18comic 下载入库后 tag 是否显示。
- [ ] 1.1.74：多项并行下载是否还卡死、单项暂停/继续/取消是否生效、下载项卡片 UI 观感。
- [ ] 1.1.77：重启后 18comic 是否直接是登录态；并行下载是否还会「导入失败」；下载完成后列表是否自动出现新画集；织机/exhentai 报错是否自动重试；exhentai 是否带 tag。
- [ ] 1.1.75：18comic / nhentai / exhentai 三站下载是否恢复；关掉画匣再打开，站点登录态是否还在；nhentai 是否还会被弹到广告页；设置→浏览器数据→登录态 粘贴 Cookie 是否立刻生效。

## 七、新窗口开场

> 「读 `D:\Qoderwork\HANDOFF.md`，接着做画匣的待办，第一条是壁纸锁拖动。」

或直接说「继续画匣的待办」，并让我先跑 `board.mjs check --project huaxia` 核对。

## 八、凭据约定

交接文本、代码、日志、广播里**都不写 cookie / token / 密码**。登录态只存在 WebView2 profile 里，
通过 `cookies_for_url` 读取，不落盘明文。
