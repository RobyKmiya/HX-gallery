# 画匣（HX-gallery）交接单

> 生成：2026-11-26 15:10 ｜ 交接人：workbuddy
> 新窗口接手：先读这份，再跑 `node D:/DSH/agent-dispatch/board.mjs check --project huaxia` 核对状态。

## 一、一句话现状

画匣已装 **1.1.69**（FileVersion 1.1.69 / 19,660,288 B / mtime 2026-11-26T06:54:36），
源码 HEAD `da3deb9`，工作区干净，看板登记一致（`check` 全绿）。浏览器广告拦截、元素选择器、
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

## 四、待办（按优先级，管理员 2026-11-26 原话）

1. **壁纸锁拖不动**：壁纸锁界面顶部留一条可拖动区域（只给拖动权限，不影响解锁）。
2. **元素屏蔽没记忆**：管理员反馈重启后屏蔽失效。排查方向：
   - `blocked.json` 是否写入（`__xkReqQ` 的 `blockElement` 是否到达 `handle_toolbar_action`）；
   - 注入只挂在 `on_page_load Finished`，**SPA 路由不会重注入** → 需要像地址栏那样监听 pushState 后补注入。
3. **nhentai tag 带数字**：nhentai 的 tag 常跟作品数，入库时剥掉，只留 tag 名。
4. **18comic 被误屏蔽、看不了画廊**：怀疑元素选择器误伤画廊容器；加保护 + 支持批量恢复。
5. **18comic 记不住账号密码**：每次要重登，账号簿要支持保存/自动填充。
6. **屏蔽元素管理 UI 不清、与广告屏蔽位置重合**：拆独立分区，入口明确。
7. **屏蔽网址功能未上线**：域名黑名单后端已有（`adblock.txt` + 设置页自定义名单），缺「按网址屏蔽」的 UI 入口。
8. **18comic album（整本）合并**：现在只支持 `/photo/` 单章；`/album/` 需章节列表 → 逐章抓图 → 合并一个 CBZ。
9. **18comic 打散切片**（条件性）：若 1.1.69 后仍错位，说明该站图片是打散切片，需按 DOM 位置/CSS 还原。
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

## 七、新窗口开场

> 「读 `D:\Qoderwork\HANDOFF.md`，接着做画匣的待办，第一条是壁纸锁拖动。」

或直接说「继续画匣的待办」，并让我先跑 `board.mjs check --project huaxia` 核对。

## 八、凭据约定

交接文本、代码、日志、广播里**都不写 cookie / token / 密码**。登录态只存在 WebView2 profile 里，
通过 `cookies_for_url` 读取，不落盘明文。
