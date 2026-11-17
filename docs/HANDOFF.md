# 画匣（Archive Gallery）工程交接文档

> 快照时间：2026-11-17 18:00（本地时间）
> 交接原因：当前会话额度用尽，交由下一位 agent / 开发者继续。

---

## 0. 一页速览

| 项目 | 内容 |
| --- | --- |
| 产品名 | 画匣（Archive Gallery） |
| 定位 | 完全本地 / 离线的桌面画集画廊，管理 ZIP / CBZ 压缩包 |
| 工程路径 | `D:\Qoderwork` |
| 技术栈 | Tauri 2 + React 19 + TypeScript strict + Vite 8 + Rust / rusqlite |
| 版本 | `1.0.0`，标识 `com.archivegallery.desktop` |
| 数据库 | SQLite，`user_version = 7` |
| 当前阶段 | **M1–M23 全部完成**，功能规划无遗留 |
| 最近改动 | M23 标签管理增强（标签内画集预览、批量导入、筛选用法说明、详情抽屉按大类分组） |
| 当前运行状态 | 正式版 `D:\Qoderwork\src-tauri\target\release\archive-gallery.exe` 正在运行（PID 可能变化） |
| 真实库 | `F:\Hthings\BENZI`，已索引 **170 本 / 68.4 GiB**，3 个收藏夹，22 个活动标签，2 个文件缺失 |
| 正式产物 | exe 12,069,888 字节；NSIS 安装包 5,835,311 字节（5.56 MiB），均为 2026-11-17 17:53 |
| 日志 | `docs/WORKLOG.md`、`docs/logs/`、`docs/logs/assets/` |
| CLI 文档 | `docs/AGENT_API.md` |

---

## 1. 交接前必读：用户明确约束（逐字保留）

1. **隐私与测试数据**
   > “测试如果你需要数据我有本子，我想还是不复制，我显存的画集很多，D盘可能不够，如果可以你帮我改一下目录，在这里， 有一些初步分类F:\Hthings\BENZI；”

   - 用户不希望复制真实画集；真实库体量巨大（当前 68.4 GiB）。
   - 除非用户**明确指定某个真实文件/画集**，功能开发与 UI 测试一律使用 `%TEMP%` 下的合成库。
   - 2026-11-17 用户明确指定过 `archive #170`（C108 妄想秘話）做标签配置，属于已授权的单次真实库操作。

2. **全量索引**
   > “F:\Hthings\BENZI 全量索引待软件完工之后再进行，继续完成软件”

   - 当前库已有 170 本索引，但**不要自行对 `F:\Hthings\BENZI` 发起全量扫描 / 索引**，必须等用户明确启动。
   - 扫描命令 `galleryctl scan` 与“重新扫描”按钮都会触碰真实库，使用前必须确认。

3. **备份**
   > “你要备份什么？原画集文件巨大，备份将占用极大空间”

   - 任何备份只允许复制元数据 `.gallery/library.db`（以及必要时 WAL/SHM），**绝不复制原始 ZIP/CBZ 画集**。
   - 应用迁移自动备份位于 `F:\Hthings\BENZI\.gallery\backups\`，同样只含数据库。
   - 若要在应用运行时手工备份元数据，先关闭应用，或同时复制 `library.db`、`library.db-wal`、`library.db-shm`，避免拿到不一致快照。

4. **日志习惯**
   > “记得完成内容的时候做日志，方便追溯，研究，其他agent理解学习，合作，继续工作”

   - 每完成一个阶段性功能或较大修复：新增 `docs/logs/YYYY-MM-DD-<主题>.md`，并在 `docs/WORKLOG.md` 底部日志索引追加一行，同时更新「当前状态」。
   - 日志至少包含：目标、变更、关键决策、验证方式、遗留问题、下一步。

---

## 2. 当前工程状态

### 2.1 功能完成度

M1–M23 已全部完成，核心能力：

- 导入：拖拽 ZIP/CBZ/文件夹、去重哈希、保留目录结构、封面缓存、重新扫描。
- 画廊：网格总览、分页/滚动、封面预解码无闪烁换页、`↑↓←→`/滚轮/边缘点击/自动翻页、分页状态与滚动位置。
- 筛选：标题/文件名/备注搜索（FTS5 trigram）、AND/OR/排除标签筛选、评分区间、阅读状态、仅文件缺失、收藏夹、分组（评分/标签/收藏夹）、筛选状态按库持久化。
- 详情抽屉：评分自动保存、备注、标签（单条/批量/已有标签选择/按大类分组）、收藏夹、阅读进度、删除。
- 标签中心：在用/分类层级/回收站、分类树管理、标签合并/删除/恢复、导图（缩放/平移/拖拽/搜索定位/悬停高亮）、标签内画集封面预览。
- 阅读器：键盘/滚轮/按钮/GIF 式即时换页、双页模式、缩放拖动、缩略图侧栏、书签、阅读进度与续读、最近阅读排序。
- 收藏夹：自命名/重命名/删除/排序/封面、批量加入、回收站恢复、单画廊快捷编辑、收藏位置选择。
- 浏览历史：鼠标侧键 X1/X2 与顶栏按钮按界面层级前进/后退。
- 发布：简体中文 NSIS 当前用户安装包，自动附带 `galleryctl`。

### 2.2 本轮（M23）最后改动

1. 标签中心：点击标签数量展开内联画集预览（最多 24 本），可跳转画廊筛选。
   - `src/components/TagCenter.tsx`：`toggleArchives`、`.tag-archives-panel`。
2. 详情抽屉批量导入标签：换行/中英文逗号/分号/顿号分隔，`#` 前缀清理，大小写不敏感去重，实时预览数量，`Ctrl+Enter` 导入。
   - `src/components/DetailDrawer.tsx`：`parseTagNames`、`submitBulkTags`。
3. 标签筛选用法说明：空 / AND / OR / 排除。
   - `src/components/LibraryView.tsx`：`.tag-filter-guide`。
4. 修复顶栏 `backdrop-filter` 造成筛选弹层被卡片遮挡：`.topbar { position: relative; z-index: 30; }`。
5. **详情抽屉标签按大类分组**（用户后续反馈补充）：
   - `src/components/DetailDrawer.tsx`：`groupTagsByCategory`、`.tag-groups`；「从已有标签选择」同样分组。
   - 固定顺序：语言 / 原作 / 角色 / 作者 / 女性 / 男性 / 混合 / 其他，其余自定义分类按中文名排序，空分类「未分类」置底。
   - 样式：`src/App.css` 的 `.tag-groups` / `.tag-group-head` / `.tag-picker-group`。
6. 真实库标签配置（archive #170）：
   - 新建分类：语言、原作、角色、作者、女性、混合、其他（「其他」暂为空）。
   - 绑定 19 个标签：
     - 语言：`chinese`, `translated`
     - 原作：`zenless zone zero`
     - 角色：`aria`, `nangong yu`, `sunna`
     - 作者：`xiaozi xiao`
     - 女性：`blindfold`, `blowjob`, `double penetration`, `drugs`, `filming`, `fingering`, `rape`, `squirting`, `unusual pupils`, `wings`
     - 混合：`group`, `mmf threesome`
   - 详见 `docs/logs/2026-11-17-benzi-archive-170-tag-config.md`。

### 2.3 最近验证

- `npx tsc --noEmit` 通过。
- dev + CDP 验证：
  - 空 / AND / OR / 排除：6 / 1 / 4 / 3 本（合成库）。
  - 批量导入 `萝莉, 水彩；全彩、AI\n纯爱`：识别 5、新增 4、已存在 1，CLI 计数一致。
  - 标签中心内联预览、筛选用法说明、弹层层级正常。
  - 详情抽屉与已有标签选择按大类分组；真实 archive #170 显示 6 组共 19 个标签。
- 正式版重建（17:53）后只读冒烟：archive #170 分组与数量正确。
- 本轮**未重跑** `cargo test --lib`（M23 未改后端）；接手后建议先跑一次。

---

## 3. 真实库现状（只读核对）

```
library:      F:\Hthings\BENZI
archives:     170
totalSize:    73,467,772,602 bytes（约 68.4 GiB）
tags:         22（活动）
collections:  3（收藏[默认]、AI、萝莉）
missing:      2
user_version: 7
```

缺失文件（仅记录，不要自动删除或清理记录）：

| id | 标题 | 相对路径 |
| --- | --- | --- |
| 11 | 12.05 井芹仁菜 Nina Iseri | `12.05 井芹仁菜 Nina Iseri.zip` |
| 20 | skills-deai-flavor | `skills-deai-flavor.zip` |

22 个标签中：

- 本次配置的 19 个已归入 7 个大类。
- 历史遗留 3 个未分类：`DEEPSEEK`、`大肥鱼`、`萝莉`。
- 是否给这 3 个标签归类、是否翻译新增英文标签，等用户决定。

元数据位置：

```
F:\Hthings\BENZI\.gallery\library.db          # 主数据库（WAL 模式，另有 -wal/-shm）
F:\Hthings\BENZI\.gallery\library.json        # 库标识
F:\Hthings\BENZI\.gallery\backups\            # 迁移备份（仅数据库）
F:\Hthings\BENZI\.gallery\tmp\                # 导入临时目录
```

应用配置与缓存：

```
%APPDATA%\com.archivegallery.desktop\settings.json
  # UTF-8 无 BOM，Windows 路径使用双反斜杠，例如 {"library_path": "F:\\Hthings\\BENZI"}

%LOCALAPPDATA%\com.archivegallery.desktop\covers\<library_id>\
  # 封面缓存，可删除后会重新生成
```

> 注意：`settings.json` 当前已恢复为 `F:\Hthings\BENZI`。测试临时库前务必备份该文件，测试结束后恢复（无 BOM、双反斜杠）。

---

## 4. 代码结构与关键入口

### 4.1 后端（Rust / Tauri）

| 文件 | 职责 |
| --- | --- |
| `src-tauri/src/main.rs` | 可执行入口 |
| `src-tauri/src/lib.rs` | Tauri 应用状态、命令注册、前后端桥接 |
| `src-tauri/src/db.rs` | 连接、迁移 v1–v7、标签/分类/收藏夹/回收站 SQL、分类路径规范化 |
| `src-tauri/src/importer.rs` | 导入流水线：解压探测、哈希去重、封面、进度 |
| `src-tauri/src/library.rs` | 扫描、查询（分页/排序/筛选/分组）、统计 |
| `src-tauri/src/archive.rs` | ZIP/CBZ 解析、图片枚举、封面提取 |
| `src-tauri/src/reader.rs` | 阅读器分页读取、缩略图、书签、阅读进度 |
| `src-tauri/src/translate.rs` | 标签翻译相关逻辑 |
| `src-tauri/src/fsutil.rs` / `paths.rs` / `models.rs` | 文件工具、路径、数据模型 |
| `src-tauri/src/bin/galleryctl.rs` | AI/脚本用 JSON CLI（导入、搜索、标签、收藏夹、备注、分类） |
| `src-tauri/src/tests.rs` | 集成测试（临时库） |

### 4.2 前端（React + TS）

| 文件 | 职责 |
| --- | --- |
| `src/App.tsx` | 启动、库切换、全局状态与主题 |
| `src/components/LibraryView.tsx` | 主画廊、顶栏、筛选面板、分组、分页、拖拽导入、浏览历史（M22） |
| `src/components/ArchiveCard.tsx` | 画集卡片、评分/进度徽标 |
| `src/components/DetailDrawer.tsx` | 详情抽屉：评分、备注、标签（批量 + 分组）、收藏夹、删除、阅读入口 |
| `src/components/TagCenter.tsx` | 标签中心：在用/分类层级/回收站、内联画集预览、合并/删除、导图入口 |
| `src/components/TagGraph.tsx` | 标签导图渲染与交互 |
| `src/components/Reader.tsx` | 阅读器：翻页、双页、缩放拖动、缩略图、书签 |
| `src/components/TrashCenter.tsx` | 统一回收站（画集/收藏夹/标签） |
| `src/components/CollectionEditor.tsx` | 单画廊快捷收藏编辑 |
| `src/components/CollectionNameDialog.tsx` | 新建/重命名收藏夹弹窗与常用名称芯片 |
| `src/components/ImportOverlay.tsx` / `ImportResultDialog.tsx` | 导入进度与结果 |
| `src/api.ts` | `invoke` 封装与 `assetUrl` |
| `src/types.ts` | 前后端共享 TS 类型（含 `Tag.category`） |
| `src/format.ts` / `Icons.tsx` / `Toast.tsx` / `Welcome.tsx` | 工具与通用组件 |
| `src/App.css` | 全部样式（无 CSS 框架） |

### 4.3 数据模型要点

- `tags.category` 是**分类路径字符串**（如 `语言`、`题材/画风`），不是分类 id。
- `tag_categories` 以 `path` 唯一（`COLLATE NOCASE`），`parent_id` 提供层级；`set_tag_category` 会自动创建路径。
- `archives.deleted_at` 为软删除；所有查询/统计/计数需排除回收站画集。
- `archives_fts` 为 FTS5 trigram；查询 ≥3 字符走 FTS，1–2 字符走 LIKE。
- 标签名称 `UNIQUE`（大小写敏感）；合并会把关联迁移到目标并软删除来源。
- 迁移备份只复制数据库文件，不接触原始画集。

---

## 5. 开发、构建与测试

### 5.1 常用命令（工程根目录 `D:\Qoderwork`）

```bash
npm run check        # tsc --noEmit
npm run build        # tsc + vite build
npm run tauri dev    # 开发模式
cargo check --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml --lib
npm run build:bundle # 正式 exe + NSIS（tsc/vite/cargo release/makensis）
npm run build:cli    # 只构建 release galleryctl
```

产物：

```
src-tauri/target/release/archive-gallery.exe
src-tauri/target/release/bundle/nsis/画匣_1.0.0_x64-setup.exe
src-tauri/target/release/galleryctl.exe
```

### 5.2 发布流程（重要）

1. 先运行 `npm run check`。
2. **关闭所有 `archive-gallery.exe` 进程**，否则 Windows 锁定 exe，release 链接会失败。
   ```bash
   powershell -NoProfile -Command "Get-Process archive-gallery | Select-Object Id,Path"
   taskkill //PID <pid> //F
   ```
3. `npm run build:bundle`，确认 exit code 0 且 makensis 输出安装包。
4. 记录新产物大小/时间；用合成库或用户授权的单个真实画集做只读冒烟。
5. 更新 `docs/WORKLOG.md` 与对应日志；清理临时库、测试封面缓存、临时进程。
6. 如用户需要，重新启动 release 版。

### 5.3 调试（CDP）

```bash
WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=9222" \
  "D:/Qoderwork/src-tauri/target/release/archive-gallery.exe"
```

- 页面地址：`http://tauri.localhost/`（release）或 `http://localhost:1420/`（dev）。
- `http://127.0.0.1:9222/json` 获取 target，使用 Node 24 全局 `WebSocket`，无需 `ws` 包。
- `Runtime.evaluate` 可操作 React 输入时，需用原生 setter + `input` 事件：
  ```js
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set;
  setter.call(input, "关键词");
  input.dispatchEvent(new Event("input", { bubbles: true }));
  ```
- 截图：`Page.captureScreenshot`，clip 需同时提供 `x/y/width/height`（缺字段会报 `mandatory field missing`）。

### 5.4 并行运行 / WebView2 冲突

- 症状：`WebView2 error: HRESULT(0x8007139F) 组或资源的状态不是执行请求操作的正确状态。`
- 原因：已有 `archive-gallery.exe` 占用 WebView2 用户数据目录。
- 处理：
  - 正常情况：结束残留进程后再启动。
  - 用户正在使用、需要并行测试：给 dev 指定独立目录：
    ```bash
    WEBVIEW2_USER_DATA_FOLDER="C:/Users/Administrator/AppData/Local/Temp/<temp-profile>" \
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=9222" \
    npm run tauri dev
    ```
  - 测试结束删除临时 profile；不要动用户正在运行的实例。

### 5.5 合成库测试流程

1. 用 Python 标准库生成 PNG（zlib/struct）并打包 `ZIP_STORED` CBZ。
2. 初始化临时库：`galleryctl init --library <temp>` 或启动应用选择临时目录。
3. 备份 `settings.json` → 改为临时库 → 测试 → 恢复 `F:\Hthings\BENZI`（UTF-8 无 BOM、双反斜杠）。
4. 测试结束：删除临时库、删除本次新建的 `covers\<library_id>`（按 mtime 识别，保留用户原有目录）、删除临时日志，确认无残留进程与 1420/9222 端口。

---

## 6. 已知问题与待办

### 6.1 真实库数据

- 2 个缺失文件（id 11、20），建议由用户在“筛选 → 仅文件缺失”确认是移动还是删除；不要自动清理记录。
- 3 个历史标签未分类（`DEEPSEEK`、`大肥鱼`、`萝莉`），可按用户意愿归入「其他」或单独分类。
- 19 个新标签目前只有英文名，未设置中文译名；如需要，可用 `galleryctl tags translate` 批量补译。
- 「其他」分类目前为空（预留）。

### 6.2 功能/体验可选增强

- 画廊的标签筛选弹层目前仍是平铺标签列表，未按大类分组（详情抽屉与标签中心已分组）。标签数量增多后可考虑复用 `groupTagsByCategory`。
- 标签批量导入是逐个 IPC 调用；常规数量无感，数百个时可考虑后端批量命令。
- 发布侧仍缺：代码签名、自动更新、MSI 安装包。
- 无 Git 仓库：工程尚未初始化版本控制。接手后建议 `git init` 并做一次基线提交；提交前确认不包含 `target/`、`node_modules/`、真实库路径或配置里的私人信息。

### 6.3 运行环境注意

- 用户可能同时开多个 release 实例；构建前全部关闭。
- `settings.json` 必须保持 UTF-8 无 BOM、双反斜杠，否则应用可能无法读取。
- 数据库为 WAL 模式，手工复制元数据时注意 `-wal` / `-shm`。
- 控制台编码：Git Bash 下 Rust CLI 输出正常；Python 脚本打印中文建议 `PYTHONIOENCODING=utf-8`，否则可能显示乱码。
- Windows 命令：Git Bash 下 `taskkill //PID <pid> //F`。

---

## 7. 接手建议流程（第一天）

1. 阅读顺序：
   - `docs/HANDOFF.md`（本文）
   - `docs/WORKLOG.md`「当前状态」与「日志索引」
   - `docs/logs/2026-11-17-m23-tag-management-bulk.md`
   - `docs/logs/2026-11-17-benzi-archive-170-tag-config.md`
   - `docs/AGENT_API.md`
2. 只读核对：
   ```bash
   src-tauri/target/debug/galleryctl.exe --library "F:/Hthings/BENZI" stats
   src-tauri/target/debug/galleryctl.exe --library "F:/Hthings/BENZI" tags list --limit 50
   ```
3. 跑一次 `npm run check` 与 `cargo test --manifest-path src-tauri/Cargo.toml --lib`。
4. 任何 UI 开发先在 `%TEMP%` 合成库验证；不要直接拿 `F:\Hthings\BENZI` 做实验。
5. 如需发布：先获得用户同意关闭正在运行的画匣，再 `npm run build:bundle`。
6. 完成工作后写日志、更新 WORKLOG，并清理临时环境。

---

## 8. 给下一位 Agent 的启动提示词（可直接复制）

```text
你在继续开发「画匣」桌面画廊，工程在 D:\Qoderwork。
先阅读 docs/HANDOFF.md、docs/WORKLOG.md 和 docs/AGENT_API.md，再阅读最近两份日志
（2026-11-17-m23-tag-management-bulk.md、2026-11-17-benzi-archive-170-tag-config.md）。

硬性约束：
1) 真实库是 F:\Hthings\BENZI，用户不希望复制原始画集；测试一律用 %TEMP% 合成库，
   除非用户明确指定某个真实画集。
2) 全量索引必须等用户明确启动，不要自行 scan 真实库。
3) 备份只允许复制 .gallery/library.db 元数据，绝不复制原始 ZIP/CBZ。
4) 每完成阶段工作写 docs/logs/YYYY-MM-DD-<主题>.md，并更新 docs/WORKLOG.md。
5) 发布前必须关闭所有 archive-gallery.exe，随后运行 npm run build:bundle。

当前状态：M1–M23 完成，正式版 1.0.0 已构建；最近完成详情抽屉标签按大类分组。
下一步先向用户确认要做的任务，再按上述流程开发、验证、记录。
```

---

## 9. 关键文档索引

- [工作日志与当前状态](WORKLOG.md)
- [AI/脚本接口 galleryctl](AGENT_API.md)
- [M23 标签管理增强与分组](logs/2026-11-17-m23-tag-management-bulk.md)
- [真实库 archive #170 标签配置](logs/2026-11-17-benzi-archive-170-tag-config.md)
- [M22 鼠标侧键前进后退](logs/2026-11-17-m22-side-buttons-history.md)
- [M19 GIF 式即时换页](logs/2026-11-17-m19-instant-frame-swap.md)
- [M16 发布打包](logs/2026-11-17-m16-release-packaging.md)
