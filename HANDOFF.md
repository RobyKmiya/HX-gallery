# 画匣（HX-gallery）交接单

> 交接：2026-11-27 03:07 ｜ 交接人：workbuddy（GLM）
> 本文件是**自包含**的：新代理零上下文，从这份 + 第四节的开工清单就能接手。
> 接手第一步：`node D:/DSH/agent-dispatch/board.mjs check --project huaxia` 核对登记，
> 再读 `projects/huaxia/state.json` 的 note（每轮验收后的最新差异都在里面）。

---

## 一、一句话现状

画匣已装 **1.1.84**（FileVersion 1.1.84 / mtime 2026-11-27T03:00:42），
源码 HEAD `f76abe9`（功能 `9785604` → 发版 `8c75eb2` → 文档 `f76abe9`），工作区干净，
看板登记一致。11-26 一天从 1.1.72 迭代到 1.1.84（13 版），下载链路、登录态、
单页增删都经过管理员验收（1.1.80 轮明确「非常好，顺利验收完工」）。
1.1.81–1.1.84 四轮为最新增量，**待人工验收**（清单见第六节）。

## 二、项目与工具链

| 项 | 值 |
|---|---|
| 源码 | `D:\Qoderwork`（master） |
| 安装位置 | `C:\Users\Administrator\AppData\Local\画匣\archive-gallery.exe` |
| 技术栈 | Tauri 2 + Rust（`src-tauri/src/`）+ WebView2 + React（`src/`） |
| 日常迭代发版 | `node scripts/release.mjs --fast`（3~5 分钟，补丁位 +1，自动提交 4 个版本文件） |
| 默认压体积档 | `node scripts/release.mjs`（约 23 分钟，只在正式发布用） |
| ⚠️ 发版铁律 | release.mjs **只提交 4 个版本文件**——功能代码必须自己单独 commit，且**先 commit 功能再跑发版** |
| 看板核对 | `node D:/DSH/agent-dispatch/board.mjs check --project huaxia` |
| 推水位 | `node D:/DSH/agent-dispatch/hooks/dsh-hook.mjs mark-synced --repo D:/Qoderwork` |
| 广播 | `node D:/DSH/agent-dispatch/agentctl.mjs broadcast --as workbuddy --task huaxia-tabs-145 --status done --title "…" --text "…"`（管理员要求：每完成一批实质工作**主动**广播，不等催） |
| 收尾三件套 | `projects/huaxia/state.json`（version/head/exeMtime/note）+ `projects/huaxia/BOARD.md`（头部登记+时间线+待办）+ `prompts/30-huaxia.md`（摘要），然后 `board.mjs check` 必须「与登记状态一致」 |

**环境注意**
- `cargo test` 在 Bash 通道会被 SIGTERM → **用 PowerShell 通道**跑 `cargo test` / `cargo check`。
- 出网 curl 只有 `--proxy http://127.0.0.1:30808` 能通（62140 与直连都不行）；系统代理坏端口旧账见第五节。
- 本地 `cargo check` 会改 `Cargo.lock`，记得一并提交。
- 发版脚本会「清除回收站（所有驱动器）」——管理员回收站有要恢复的东西时先提醒。
- 发版时 `export-tags.mjs` 需要**独占库文件**：画匣开着会报 readonly database，不影响安装包；关应用后重跑补翻译。
- Rust 单测现状 98 个全过；动后端必须跑 `cargo test --manifest-path src-tauri/Cargo.toml --lib`。

## 三、代码地图（新代理最容易迷路的模块）

| 模块 | 职责 |
|---|---|
| `netdl.rs` | 链接下载核心：三站解析、多任务管理器、curl 抓取、打包入库。公开给兄弟模块：`cookie_for`/`proxy_arg`/`set_running_stage`/`task_is_canceled_public`/`download_and_package`/`JmDecode` |
| `jm_decode.rs` | 18comic 图片混淆还原。**段数带画面自校验**（缩略图按相邻行差异打分，候选段数含 0 全试，公式结果不连续时改用最优）——别删这层，公式依赖的 scramble_id 随站点改版会失效 |
| `jm_api.rs` | 18comic 移动 API 路线（jmcomic 协议）：token=md5(ts+`185Hcomic3PAPP7R`)，返回 base64→AES-256-ECB（key 同 md5hex）解 JSON；API 域名 cdnhjk/cdngwc 系列按序试。域名/密钥是**站点会改的外部常量**，坏了对照 jmcomic 仓库更新 |
| `page_ops.rs` | 画廊单页增删：zip/cbz **重打包**（画集永远是压缩包，「导入文件夹」实为批量导 zip 的目录）。删除页落 `.gallery/trash/pages/{id}/`；原子替换失败自动回滚；缓存键自带 mtime+size，替换后自然失效 |
| `reader.rs` + `pagecache.rs` | 阅读器页读取 + LRU 字节缓存。缓存键带「mtime+size」，**替换包后旧键自然失效，不要手写失效逻辑** |
| `site_login.rs` | 登录态：回填按**标志 cookie** 判定（18comic=AVS / exhentai=ipb_member_id / nhentai=sessionid|access_token），名单外站点退回「任意 cookie」；`credential_from_snapshot` 从 remember_id 解明文账密给自动填充。加新站点适配时**先在这里登记标志 cookie** |
| `browser.rs` | 内嵌浏览器：标签/油猴/GM 请求轮询（当前标签每轮+后台轮转）/右键菜单/自动填充（账号簿→快照 remember_id 兜底）/广告拦截 |
| `d2.rs` | 网页下载入库（`import_downloaded_meta` 会把源 URL 写进备注）；维护锁用 `wait_maintenance`（排队 3 分钟），**不要退回 try_lock** |
| `container.rs` / `archive.rs` | zip/7z/rar 页枚举（`list_entries` 只认压缩包；`is_image_name`/`is_ignored_entry` 是页过滤白名单，复用它保证与阅读器同序） |
| `scripts/release.mjs` | 发版脚本（快/慢两档；自动校验安装、刷桌面快捷方式） |

前端关键文件：`LibraryView.tsx`（主页/侧栏/详情）、`WorkspaceTabs.tsx`（多标签 + `openUrlTab` 事件 `huaxia:open-url-tab`）、`Reader.tsx`（阅读器）、`PageOpsPanel.tsx`（图片管理，详情页与阅读器共用）、`SettingsDialog.tsx`（设置，含「阅读」页）、`BrowserDataSettings.tsx`（登录态粘贴块）。

## 四、开工清单（管理员的最新一批，2026-11-27 02:19–03:07）

动效批复已齐（A1 撤/A5 已有/A9–A15 过/A16 可选已落设置页/A6 视觉改高亮脉冲+补打包进度事件），
**动效工程统一实施是下一件大事**；两个已批复方案可直接动工：

1. **UI 动效统一实施**（工程后延中，批复已齐）：A2 交错入场 / A3 面板过渡 / A4 抽屉滑入 /
   A6 高亮脉冲进度（从左往右扫光 + **打包阶段补进度事件**，当前打包期进度会突变） /
   A7 反馈弹跳（幅度再夸张）/ A8 Toast / A9 分区指示条滑动 / A10 搜索框聚焦 /
   A11 列表项收缩消失 / A12 计数滚动 / A13 右键菜单浮现 / A14 分组展开收起 /
   A15 壁纸交叉淡化。A1 卡片悬停**不做**（管理员自有想法）；A16 翻页过渡已做成可选开关
   （设置→阅读）。预览动效的参考实现都在 2026-11-27 会话的两批 show_widget 里。
2. **心仪画集「镭射立体卡」范式一**：交互预览已交管理员（鼠标跟随倾转 + 镭射渐变
   color-dodge + 高光斑随光标 + 文字 translateZ 抬升），**待批复手感**后落到主页网格与收藏夹
   视图；实现要点在预览脚本里。只给「心仪」级画集用，普通网格保持平面卡。
3. **收藏夹合并第二步**：散图已并入侧栏收藏夹折叠组（1.1.83）；下一步做聚合视图
   （进去后分 签/散图/画廊/画集 子页，结构留影匣视频扩展位）。
4. **批量 e 站反查源链接**（管理员点名）：拿本地画集标题去 e 站搜索，优先中文、评分高、
   收藏多，人工确认后写进备注「源：」行。需要登录态 + 搜索 API + 确认 UI。
5. 旧待办仍有效的：元素屏蔽记忆（重启失效，怀疑 SPA 不重注入）、nhentai tag 带数字剥除、
   18comic 误屏蔽恢复、屏蔽网址 UI 入口、断点续传、rememberWindowPosition、新页标签美化、
   标签行为向 Edge 看齐、F12 开发者面板、更新画匣整合进设置 + GitHub 推流自动检查。

## 五、踩过的坑（每一条都真实翻过车，别重犯）

**下载链路**
1. 坏代理端口：老实现环境变量优先于系统代理，`HTTPS_PROXY` 指向失效端口打挂全部下载。
   `netdl::proxy_arg` 已改**系统代理优先，别改回去**。
2. Cloudflare：curl 直连 18comic/nhentai 必 403（需要 cf_clearance，且绑 UA+TLS 指纹）。
   抓页走 `fetch_html_smart`（curl 失败→**已登录标签内同源 fetch**）。API 路线（jm_api）不受 CF 影响。
3. 图片判定用**文件头魔数**，不要信 Content-Type（图床超时返回 200 的 HTML 页）。
4. 18comic 图片必须按 `<img>` DOM 顺序取，分属性两轮抓会乱页序。
5. 18comic 段数公式依赖 scramble_id（页面可能取不到/取错）→ 必须保留 jm_decode 的画面自校验层。
6. 加密 zip 重打包后新包**不加密**（界面已有提示）；7z/RAR 不支持单页增删。

**登录态**
7. 回填判定必须按**标志 cookie**（AVS 等），不能用「有没有任意 cookie」——
   theme/_ga/cf_chl 是持久 cookie，重启还在，会永远误判已登录（1.1.77 前的核心 bug）。
8. AVS 是会话 cookie 且服务端会过期（改密码吊销全部会话）——注入救不回过期 cookie，
   只能引导用户在标签里重登一次让自动快照重存。

**浏览器引擎**
9. WebView2 装不了扩展（无 uBlock）；油猴脚本烘焙在窗口创建时注入，改完必须重开窗口；
   `ensure_builtin_scripts` 必须「内容不同就覆盖」，不能「存在就跳过」。
10. SPA 路由（pushState）不触发引擎 on_navigation——地址栏回填靠 hook history。
11. **双击点歪**：点击后布局会变的组件（点卡片弹详情抽屉）上，双击会因网格重排落空——
    解法是单击延迟 240ms、双击取消单击。已在 ArchiveCard 实现。
12. 沉浸阅读右下角还原点已移除（管理员要求）；退出途径：F / 右键菜单 / 鼠标后退侧键 / Esc。

**Rust/Tauri 工程坑（今天踩的）**
13. 在某个 `#[tauri::command]` fn **前**插入新命令时，老 fn 的属性行会被新块抢走，编译报
    `__tauri_command_name_xxx not found`——插入后检查属性归属。
14. zip 2.x 压缩方法在 `zip::CompressionMethod`（`zip::compression` 私有）；ecb crate 要开
    `features=["std"]` 才有 `decrypt_padded_vec_mut`。
15. `fsutil::modified_ms` 收 `&Metadata` 不是 `&Path`。
16. `&AppHandle` 进 `thread::spawn(move||)` 前先 clone，否则生命周期逃逸。
17. tauri command 的维护锁排队用 `wait_maintenance`（try_lock 立即失败曾导致并行下载
    「导入失败还得重下」）。

**前端**
18. 跨组件共享阅读偏好：Reader 用 `usePersistentFlag`、SettingsDialog 用 PrefToggle 直写
    localStorage 同一键（如 `huaxia.reader.showPageHud`）。
19. 详情页/阅读器改了画集 → `emit("d2://download", {ok:true})` 触发主页防抖刷新，别层层传回调。
20. 发版前**必跑**：`cargo check` + `cargo test --lib` + `npm run check` + `npm run build`。

## 六、待管理员验收（1.1.81–1.1.84，全部已装机）

- [ ] 详情页备注区「源：…」链接点击是否在新网页标签打开原页（1.1.81）。
- [ ] 读一次漫画后顶部是否只有一层框（1.1.81 修 Reader 重开原生标题栏）。
- [ ] 壁纸锁：滚轮缩放/拖拽平移/双击复位；无壁纸或加载失败是否可见提示（1.1.81）。
- [ ] 双击画廊是否还点歪（详情停靠挤出/辅屏）（1.1.82）。
- [ ] 侧栏清爽度、链接下载改名、设置齿轮、设置→通用→当前库 的库操作（1.1.82）。
- [ ] 详情页图片管理：删中间一页→阅读器页数同步、`.gallery/trash/pages/` 可见被删页、
      追加图片到末尾（1.1.83）。
- [ ] 侧栏收藏夹折叠组里能看到散图收藏夹（1.1.83）。
- [ ] 阅读器工具栏图片角标 → 图片管理浮层（1.1.84）。
- [ ] 沉浸阅读右下角无按钮，F/右键/侧键/Esc 退出正常（1.1.84）。
- [ ] 设置→阅读 两个开关生效（1.1.84）。
- [ ] 镭射立体卡范式预览手感批复（预览在会话里，未实现）。
- [ ] 补跑 `scripts/export-tags.mjs`（需先关画匣，库被占）。

## 七、版本史速查（1.1.72 → 1.1.84，细节见 BOARD 时间线）

| 版本 | 一句话 |
|---|---|
| 1.1.72–73 | 18comic 混淆还原初版 + nhentai v2 API + 标签保状态 |
| 1.1.74 | 下载管理器：多任务并行/单项启停/防卡死 |
| 1.1.75 | 下载全挂/登录态丢失/广告劫持三连修（魔数嗅探/真实 UA/同源 fetch/自动快照） |
| 1.1.76 | 18comic 段数画面自校验 + 浏览器性能治理（GM 轮询轮转等） |
| 1.1.77 | 标志 cookie 回填 + 导入排队不丢 + 自动重试 + exhentai tag 织机解析 |
| 1.1.78 | 登录态快照可见化（cookie 名 + 缺标志提示） |
| 1.1.79 | 「Edge 打开」试水口（管理员后拍板不接外接浏览器，一窗独有） |
| 1.1.80 | 18comic 移动 API 源数据路线 + 登录框自动填充账密（**管理员验收通过**） |
| 1.1.81 | 源链接进详情页可点回原页 + 修双框 + 壁纸锁拖拽缩放 |
| 1.1.82 | 修双击点歪 + 改名链接下载 + 设置齿轮 + 侧栏减负 |
| 1.1.83 | 画廊单页增删（重打包+回收站）+ 散图并入收藏夹组 |
| 1.1.84 | 图片管理进阅读器 + 沉浸去还原点 + 设置「阅读」页 |

## 八、新窗口开场

> 「读 `D:\Qoderwork\HANDOFF.md` 交接单，先跑 `board.mjs check --project huaxia` 核对，
> 然后开工第四节第 1 条（UI 动效统一实施）。」

管理员习惯：反馈常带截图（文件会落 `C:\Users\Administrator\.workbuddy\clipboard-images\`
或 blobs，没落盘时按文字描述做）；喜欢当天闭环、每轮发版可验；重要架构决策会自己拍板
（如「不接外接浏览器，一窗独有」「动效逐项预览批复」）。

## 九、凭据约定

交接文本、代码、日志、广播里**都不写 cookie / token / 密码**。登录态存 WebView2 profile
与 DPAPI 加密库；`remember_id` 里虽有明文账密，只用于登录框自动填充，不外显、不落日志。
（已提醒管理员：贴进聊天的那份 18comic cookie 含明文密码，建议改密。）
