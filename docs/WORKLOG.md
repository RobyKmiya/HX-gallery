# 工作日志

本项目的工作日志用于追溯开发过程、记录决策与验证结果，并帮助后续 agent 快速接手。

## 使用约定

- 每完成一个阶段性功能或一次较大修复，新增一份 `docs/logs/YYYY-MM-DD-<主题>.md`。
- 在本文件底部「日志索引」追加一行摘要。
- 日志至少包含：目标、变更、关键决策、验证方式、遗留问题、下一步。
- 提交前更新「当前状态」，避免后续 agent 依据过期信息工作。

## 当前状态

- 阶段：**M23 已完成**（M1–M23 功能规划完成，M23 为标签管理增强：标签中心每个标签可直接展开画集封面预览并跳转筛选，详情抽屉支持一次批量导入 n 个标签（换行/逗号/分号/顿号，重复自动跳过），详情抽屉已选标签与「从已有标签选择」按大类分组显示，标签筛选弹层补充「空 / AND / OR / 排除」用法说明；Windows NSIS 安装包与正式版本 1.0.0）
- 可运行：`npm run tauri dev`
- **发版：`npm run release`**（自动抬版本 + 三处同步 + 检查 + 测试 + 关实例 + 打包 + 静默安装 + 验证）。桌面「画匣」指向 `%LOCALAPPDATA%\画匣\archive-gallery.exe`，安装即覆盖，跑完就是最新版
- 本轮新增（2026-11-18）：**阅读器连读**（阅读器顶栏与页脚「上一本 / 下一本」，快捷键 `[` / `]`，切换不新压浏览历史；顺手修复焦点在按钮上按空格既翻页又吞掉按钮激活的老问题）；**待看目录**（按标签权重加权随机挑 N 本排队，默认 10；目录最多 3 个、默认以当天日期命名、带「已读完 / 总数」进度条；标签权重可在标签中心逐条设置，默认 100，**0 表示整本排除带此标签的画集**；挑过的本按 `read_count` 降权，看得越少越容易进队列；同名目录再次生成只补差额；侧栏可进入某个目录按队列筛选画廊）；**单实例守卫**（Win32 命名互斥体，第二个实例自动退出并唤醒已有窗口；关窗后进程确实退出、exe 立即释放），并已重新打包安装到 `%LOCALAPPDATA%\画匣\`、修正桌面快捷方式
- 已验证：导入、去重、扫描、封面缓存、网格总览、排序、滚动加载、详情抽屉、评分编辑、标签编辑、收藏夹创建/加入/移除/重命名/删除/排序/封面/批量加入/回收站恢复与彻底删除、标签中心（编辑/搜索/合并/回收站）、标签导图（滚轮光标锚定缩放、按钮缩放百分比、拖动平移、双击空白重置、节点点击筛选、一级分类配色与图例、节点拖拽重排、悬停邻居/连线高亮、标签搜索与 Enter 定位、适应窗口）、分页搜索（标题/文件名/备注）、备注编辑、AND/OR/排除筛选、拖拽 ZIP/CBZ/文件夹导入、卡片快捷收藏编辑器、筛选状态按库记住、按评分分组（组头数量、搜索叠加、刷新保持）、内置阅读器（快捷入口、键盘翻页、页码跳转、缩放/拖动、续读进度、双页模式、书签收藏/跳转/删除/持久化、缩略图侧栏懒加载/点击跳页/双页双高亮）、标签分类层级（多级分类树、新建子分类/重命名/移动/删除、标签行内分类下拉、搜索保留分类链）、按标签分组（主标签规则、筛选优先、未分类置底、组头分类路径、刷新保持）、最近阅读排序（未读恒置底）、卡片阅读进度徽标与封面进度条、详情抽屉阅读进度与「继续/重新阅读」入口、阅读器进度实时回写、按收藏夹分组（主收藏夹规则、默认收藏夹优先、未收藏置底、浏览收藏夹时归入当前收藏夹、成员变更即时重分组、搜索叠加、刷新保持）、画集软删除与统一回收站（详情抽屉/批量两段式删除、画集/收藏夹/标签三页签、恢复保留成员关系与进度、彻底删除仅清记录与封面缓存、所有查询/统计/标签图/收藏夹计数排除回收站画集）、组合筛选（评分区间桶、阅读状态、仅文件缺失，与标签/收藏夹/搜索/分组按维度交集叠加，激活条件芯片与一键清除、空结果条件摘要、按库持久化，CLI `--rating-buckets/--read-status/--missing`）、发布打包（版本 1.0.0、产品名「画匣」、应用图标、简体中文 NSIS 当前用户安装包、自动 external binary 附带 `galleryctl`、静默安装/启动/卸载验证、安装包内无重复资源）、画廊分页增强（封面预解码后再替换旧页、换页滚动复位、分页加载指示、左右边缘点击带、`↑↓←→` 翻页、滚轮水平/Shift+滚轮/顶部向上/底部向下翻页、自动翻页 5/8/12 秒可暂停与末页循环、自动控件常驻顶栏、按库记住间隔）、GIF 式即时换页（阅读器每页解码缓存/在途合并/前后各预取、双页原子提交、舞台滚动复位；画廊页面数据缓存同步提交、±1 页预取、前后页封面 DOM 驻留层防解码位图回收，正式构建换帧 9–10 ms 且首帧未解码封面为 0，前后翻页与已访问页回翻均无空白/模糊/位移）、收藏夹自定义命名与收藏位置选择（统一命名弹窗与全彩/黑白/萝莉/AI 常用芯片、24 字上限与大小写不敏感重名校验、侧栏/收藏夹页/批量工具条三处入口、详情抽屉内联新建即收藏、批量新建后自动选中新收藏夹、卡片心形入口提示「收藏到…」）、鼠标滚轮翻页兼容（阅读器普通滚轮上下/左右翻页、放大后先滚画面到边界再翻页、Ctrl+滚轮仍缩放；画廊横向拨轮/Shift+滚轮/边轨区域翻页、上下滚轮列表滚到边界翻页、deltaMode 归一化与手势防连翻）、鼠标侧键前进后退（顶栏后退/前进按钮与 X1/X2 侧键按界面层级移动：全部画集↔收藏夹↔详情抽屉↔阅读器↔标签中心↔回收站，快照去重、前进分支截断、250ms 侧键去重、两端禁用态）、标签管理增强（标签中心数量按钮展开内联画集预览：封面/评分/页数/缺失标记、最多 24 本与「在画廊中筛选」入口；详情抽屉批量导入标签：换行/中英文逗号/分号/顿号分隔、`#` 前缀清理、大小写不敏感去重、实时预览识别/新增/已存在数量、Ctrl+Enter 导入、跳过已存在并 toast 汇总；详情抽屉已选标签与「从已有标签选择」按 语言/原作/角色/作者/女性/混合/其他/未分类 分组并显示每组数量；标签筛选弹层新增空/AND/OR/排除四行用法说明；修复顶栏 backdrop-filter 层叠上下文导致筛选弹层被画集卡片遮挡的问题）、阅读器连读（顶栏与页脚上/下一本、`[`/`]`、切本不压历史、键盘守卫排除 BUTTON 等）、待看目录（加权随机挑选、0 权重整本排除、读得少优先、上限 3 个、日期命名、进度条、按队列筛选）
- 未实现：无（M1–M23 功能规划全部完成）；发布侧可选：代码签名、自动更新、MSI 安装包；待看目录的手动增删单本、标签权重批量设置尚未做
- 数据库版本：`user_version = 8`（v8 为 `archives` 新增 `read_count`、为 `tags` 新增 `weight`（默认 100），并新增 `queues` / `queue_archives` 两张待看目录表；v7 之前见各期日志。旧库下次打开自动迁移。迁移备份**只复制 `.gallery/library.db` 这个元数据文件**到 `.gallery/backups/`，不会复制任何原始 ZIP/CBZ 画集）
- 默认库设置：`F:\Hthings\BENZI`（原地索引 2 个样本；当前仍是 schema v5，下次由新版本打开会连续迁移到 v7 并备份；v3→v5 的备份为 `.gallery/backups/library-v3-1794844676199.db`；演示库已清理；`settings.json` 已修正为合法 JSON：UTF-8 无 BOM、路径双反斜杠）
- AI 接口：`galleryctl`，详见 [docs/AGENT_API.md](AGENT_API.md)

## 快速验证

```bash
npm run check          # TypeScript 类型检查
npm run build          # 前端构建（注：只编 dist，不重编 Rust）
cargo check --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml --lib
npm run tauri dev      # 启动桌面应用

# ★ 发布更新：一条命令搞定「抬版本 → 检查 → 测试 → 关实例 → 打包 → 安装 → 验证」
npm run release        # 补丁位 +1
npm run release:minor  # 次版本 +1
npm run release:major  # 主版本 +1

npm run build:bundle   # 只打包不安装（一般不用，优先上面的 release）

# AI 接口自检
cargo build --manifest-path src-tauri/Cargo.toml --bin galleryctl
src-tauri/target/debug/galleryctl.exe --library "F:/Hthings/BENZI" stats
```

## 关键入口

- **交接文档（先读这个）**：[docs/HANDOFF.md](HANDOFF.md) — 工程现状、约束、构建、调试、发布、已知问题
- 后端命令与应用状态：`src-tauri/src/lib.rs`
- 数据库与迁移：`src-tauri/src/db.rs`
- 导入流水线：`src-tauri/src/importer.rs`
- 扫描与查询：`src-tauri/src/library.rs`
- 压缩包解析与封面：`src-tauri/src/archive.rs`
- AI JSON 接口：`src-tauri/src/bin/galleryctl.rs`（说明见 `docs/AGENT_API.md`）
- 前端启动与库切换：`src/App.tsx`
- 画廊主界面（拖拽导入、筛选持久化）：`src/components/LibraryView.tsx`
- 单画廊快捷收藏编辑：`src/components/CollectionEditor.tsx`
- 统一回收站：`src/components/TrashCenter.tsx`
- 内置阅读器：`src/components/Reader.tsx`、`src-tauri/src/reader.rs`
- 标签中心：`src/components/TagCenter.tsx`
- 待看目录：`src/components/QueuePanel.tsx`、`src-tauri/src/queue.rs`（加权随机挑选）
- 详情抽屉：`src/components/DetailDrawer.tsx`
- 集成测试：`src-tauri/src/tests.rs`
- 发布配置：`src-tauri/tauri.conf.json`、`src-tauri/icons/app-icon.png`

## 日志索引

- [2026-11-16 · M1 框架搭建](logs/2026-11-16-m1-framework.md)
- [2026-11-16 · M2 评分编辑](logs/2026-11-16-m2-rating-edit.md)
- [2026-11-16 · M2 标签与收藏夹](logs/2026-11-16-m2-tags-collections.md)
- [2026-11-16 · M2 标签中心、自动翻译与导图](logs/2026-11-16-m2-tag-center.md)
- [2026-11-16 · M3 标签接口、回收站、搜索备注、收藏夹管理](logs/2026-11-16-m3-search-tags-collections.md)
- [2026-11-16 · M4 拖拽导入、单画廊快捷收藏、筛选持久化](logs/2026-11-16-m4-drag-drop-quick-edit.md)
- [2026-11-16 · M5 按评分分组展示](logs/2026-11-16-m5-grouping.md)
- [2026-11-16 · M6 收藏夹软删除与回收站](logs/2026-11-16-m6-collection-trash.md)
- [2026-11-16 · M7 内置阅读器](logs/2026-11-16-m7-reader.md)
- [2026-11-16 · M8 标签导图缩放平移](logs/2026-11-16-m8-tag-graph-zoom.md)
- [2026-11-16 · M9 阅读器双页模式与书签](logs/2026-11-16-m9-reader-spread-bookmarks.md)
- [2026-11-17 · M10 阅读器缩略图侧栏](logs/2026-11-17-m10-reader-thumbnails.md)
- [2026-11-17 · M11 标签分类层级与按标签分组](logs/2026-11-17-m11-tag-categories-grouping.md)
- [2026-11-17 · M12 最近阅读排序与阅读进度展示](logs/2026-11-17-m12-recent-reading-progress.md)
- [2026-11-17 · M13 按收藏夹分组](logs/2026-11-17-m13-collection-grouping.md)
- [2026-11-17 · M14 统一回收站](logs/2026-11-17-m14-unified-trash.md)
- [2026-11-17 · M15 组合筛选](logs/2026-11-17-m15-combined-filters.md)
- [2026-11-17 · M16 发布打包](logs/2026-11-17-m16-release-packaging.md)
- [2026-11-17 · M17 标签导图交互与视觉增强](logs/2026-11-17-m17-tag-graph-enhancements.md)
- [2026-11-17 · M18 画廊分页无闪烁与多输入翻页](logs/2026-11-17-m18-paging-flicker-inputs.md)
- [2026-11-17 · M19 GIF 式即时换页](logs/2026-11-17-m19-instant-frame-swap.md)
- [2026-11-17 · M20 收藏夹自定义命名与收藏位置选择](logs/2026-11-17-m20-collection-create-picker.md)
- [2026-11-17 · M21 鼠标滚轮翻页兼容](logs/2026-11-17-m21-wheel-paging.md)
- [2026-11-17 · M22 鼠标侧键前进后退](logs/2026-11-17-m22-side-buttons-history.md)
- [2026-11-17 · M23 标签管理：画集预览、批量导入与筛选说明](logs/2026-11-17-m23-tag-management-bulk.md)
- [2026-11-17 · 真实库标签配置：archive #170](logs/2026-11-17-benzi-archive-170-tag-config.md)
- [2026-11-18 · 真实库标签中文译名批量写入](logs/2026-11-18-tag-translation-zh.md)
- [2026-11-18 · 阅读器连读与待看目录（v8）](logs/2026-11-18-reader-next-and-queue.md)
- [2026-11-18 · 单实例守卫与更新不生效根因](logs/2026-11-18-single-instance-and-update-flow.md)
- [2026-11-18 · 一键发布脚本（自动抬版本+打包+安装）](logs/2026-11-18-one-command-release.md)
