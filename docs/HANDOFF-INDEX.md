# 画匣 · 交接目录

> 交接对象：接手这个工程的任何人（含 AI）。
> 生成日期：2026-11-18
> **本目录是唯一入口。** 下面的「必读顺序」读完，你就具备独立接手的能力。

---

## 一、5 分钟上手

```bash
cd D:\Qoderwork

npm run check                    # 前端类型检查
cargo test --manifest-path src-tauri/Cargo.toml --lib   # 后端测试（35 通过）
npm run tauri dev                # 开发模式

npm run release                  # ★ 发布：抬版本→检查→测试→关实例→打包→安装→验证
```

**桌面快捷方式指向**：`%LOCALAPPDATA%\画匣\archive-gallery.exe`（安装版）。
**不要**双击 `src-tauri/target/release/archive-gallery.exe`——那是构建产物，`npm run build` 不会更新它。

---

## 二、必读顺序

| 顺序 | 文件 | 为什么 |
| --- | --- | --- |
| 1 | 本文 | 全局地图 |
| 2 | [HANDOFF.md](HANDOFF.md) | 工程约束、构建细节、已知坑（**最权威**） |
| 3 | [WORKLOG.md](WORKLOG.md) | 当前状态 + 全部日志索引 |
| 4 | [AGENT_API.md](AGENT_API.md) | CLI `galleryctl` 字段级约定 |
| 5 | [AUDIT-2026-11-18.md](AUDIT-2026-11-18.md) | 本轮审计结论与待办 |

改动前请先读 `HANDOFF.md`，里面有硬约束（真实库不可碰、备份只许元数据等）。

---

## 三、工程事实（2026-11-18 实测）

| 项 | 值 |
| --- | --- |
| 版本 | **1.0.11**（`package.json` / `Cargo.toml` / `tauri.conf.json` 三处必须一致） |
| 技术栈 | Tauri 2.12 + React 19 + Rust 1.99 / SQLite |
| 数据库版本 | `user_version = 8` |
| 真实库 | `F:\Hthings\BENZI`，**已迁移到 v8**，230 个标签（226 条有中文名） |
| 后端代码 | 约 11,500 行（`src-tauri/src`，16 个模块） |
| 前端代码 | 约 10,500 行（`src`，16 个组件）+ `App.css` 5,838 行 |
| 测试 | 35 通过 / 1 ignored（性能基线） |
| 质量门禁 | `npm run check` 通过；clippy 20 条警告（历史基线，未新增） |
| Git | 有仓库，`master` 分支，**29 个文件未提交**（见审计报告） |

---

## 四、代码地图

### 后端 `src-tauri/src/`

| 文件 | 行数 | 职责 |
| --- | --- | --- |
| `db.rs` | 2648 | 连接、迁移、**所有表操作**（最大文件） |
| `lib.rs` | 1368 | Tauri 命令注册、应用状态、设置读写 |
| `library.rs` | 881 | 扫描、查询（`query_archives` 拼 SQL） |
| `tests.rs` | 2260 | 集成测试（35 个） |
| `importer.rs` | 712 | 导入流水线 |
| `queue.rs` | 463 | **待看目录：加权随机挑选** |
| `models.rs` | 369 | 全部结构体（camelCase 序列化） |
| `release.rs` | 332 | **退出后自动发布** |
| `reader.rs` | 329 | 阅读器页面缓存 |
| `archive.rs` | 277 | ZIP/CBZ 解析、封面 |
| `fsutil.rs` | 182 | 路径工具 |
| `pagecache.rs` | 170 | 页面字节 LRU |
| `instance.rs` | 129 | **单实例互斥体** |
| `translate.rs` | 131 | 标签词典（131 行硬编码） |
| `asset_protocol.rs` | 105 | `gallery://` 协议 |
| `main.rs` | 6 | 入口 |

### 前端 `src/`

| 文件 | 行数 | 职责 |
| --- | --- | --- |
| `components/LibraryView.tsx` | 2954 | **主界面**（最大，52 个 useState） |
| `components/TagCenter.tsx` | 1426 | 标签中心 |
| `components/Reader.tsx` | 1276 | 阅读器 |
| `components/DetailDrawer.tsx` | 1128 | 详情抽屉 |
| `components/TagGraph.tsx` | 705 | 标签导图 |
| `components/ImportDialog.tsx` | 550 | 导入面板 |
| `components/TrashCenter.tsx` | 377 | 回收站 |
| `components/QueuePanel.tsx` | 369 | **待看目录面板** |
| `components/CollectionEditor.tsx` | 293 | 收藏编辑 |
| `components/ReleasePanel.tsx` | 288 | **更新面板** |
| 其他 | — | `ArchiveCard` / `Icons` / `Toast` / `Welcome` 等 |
| `App.css` | 5838 | 全部样式（无 CSS 框架） |

### CLI

`src-tauri/src/bin/galleryctl.rs`（837 行）——`tags` / `collections` / `queue` / `search` / `archives` / `scan` / `import` / `stats`。

---

## 五、数据库结构（v8）

```
archives          id, rel_path, file_name, ext, file_size, file_mtime, page_count,
                  title, rating(默认60), note, cover_key, content_hash, added_at,
                  last_read_at, last_page, read_count(★v8), missing, deleted_at, error
tags              id, name, original_name, translated_name, translation_source,
                  translated_at, category, deleted_at, merged_into, weight(★v8, 默认100)
archive_tags      (archive_id, tag_id)
collections       id, name, is_default, created_at, sort_order, cover_archive_id, deleted_at
collection_archives (collection_id, archive_id, added_at)
queues            id, name, created_at                              ★v8
queue_archives    (queue_id, archive_id, added_at)                  ★v8
tag_categories    id, name, parent_id, path(唯一, NOCASE), created_at
reader_bookmarks  id, archive_id, page, created_at
archives_fts      FTS5 trigram（title / file_name / note）
```

**关键语义**
- 软删除：`archives.deleted_at`、`tags.deleted_at`、`collections.deleted_at`；**所有查询都必须排除回收站**。
- `tags.weight = 0` 表示「不想看带此标签的画集」→ 待看目录**整本排除**（不是计 0 权重）。
- `read_count` 在 `open_reader` 时自增，用于「看得少优先」。

---

## 六、本轮（2026-11-18）新增的三块功能

### 1. 阅读器连读
- `Reader.tsx`：顶栏 + 页脚「上一本 / 下一本」，快捷键 `[` / `]`
- `LibraryView.tsx`：`readerIndex` + `switchReaderArchive`
- 切换**不新压浏览历史**（否则后退会退回上一本而非退出阅读器）
- 顺手修了空格键在按钮上被翻页劫持的老问题

### 2. 待看目录（`queue.rs` + `QueuePanel.tsx`）
- 加权随机挑 N 本（默认 10），权重 = 标签权重和 ÷ (1 + read_count)
- 最多 3 个目录，超出淘汰最旧的；默认以当天日期命名
- 同名目录再次生成只补差额；进度条按 `last_page >= page_count` 算
- 随机源自写 SplitMix64（工程无 `rand` 依赖）

### 3. 退出后自动发布（`release.rs` + `ReleasePanel.tsx`）
- 面板登记源码目录 → 关窗时生成脚本 → 脚本脱离进程树等应用退出 → 跑 `npm run release` → 重开
- **编码坑**：`.cmd` 必须纯 ASCII，中文路径交给 node 助手（`release-helper.mjs`）；加 `chcp 65001` 会让 cmd 把 `setlocal` 解析成 `01`
- **黑框坑**：explorer 直接拉 `.cmd` 会带出可见控制台，一挂几分钟；改为 explorer 拉 `release-on-exit.vbs`（UTF-16LE+BOM，wscript 才认），由它 `sh.Run "...cmd...", 0, False` 隐藏执行整条链路

### 4. 单实例守卫（`instance.rs`）
- Win32 命名互斥体 `Local\com.archivegallery.desktop.single-instance`
- **坑**：名字已存在时 `CreateMutexW` 也返回有效句柄，必须查 `GetLastError() == ERROR_ALREADY_EXISTS`
- 唤醒用 `EnumWindows` 按标题前缀找（Tauri 2 窗口类名不是产品名）

---

## 七、硬约束（违反会出事）

1. **不要复制真实画集**。`F:\Hthings\BENZI` 只许读，UI 测试一律用 `%TEMP%` 合成库。
2. **不要自行全量扫描真实库**，必须等用户明确要求。
3. **备份只许复制元数据**（`library.db` + `-wal` + `-shm`），绝不复制 ZIP。
4. **`settings.json` 必须 UTF-8 无 BOM**，路径用双反斜杠。
5. **打包前必须关掉所有 `archive-gallery.exe`**，否则 exe 被锁、链接失败。
6. **三处版本号必须同步**（`npm run release` 已自动处理）。
7. 已修复的坑不要回退：阅读器竞态双令牌、评分 `saveSeqRef`、缩略图 `IntersectionObserver`、封面预解码。

---

## 八、验证方式（接手先跑一遍）

```bash
npm run check                                              # 应无输出
cargo test --manifest-path src-tauri/Cargo.toml --lib      # 应 35 passed / 1 ignored
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets  # 20 条警告（基线）
node -e "console.log(require('./package.json').version)"   # 1.0.11
```

**打开应用检查**（人工）：
1. 侧栏有「待看目录」「更新画匣」两个入口
2. 阅读器顶栏有上一本/下一本，按 `[` `]` 能切
3. 标签中心每个标签有「权重」输入框，设 0 会有红字提示
4. 关窗后进程确实退出（任务管理器无残留）

---

## 九、已知问题（接手优先处理）

见 [AUDIT-2026-11-18.md](AUDIT-2026-11-18.md) 的「待办」一节。摘要：

- **29 个文件未提交**，其中包含本轮全部功能（一旦误删即丢失）
- 根目录有个 0 字节垃圾文件 `)`
- devDependencies 有 5 个 high 漏洞（vite 链路，不影响分发）
- `LibraryView.tsx` 2954 行、`db.rs` 2648 行，拆分是长期债
- 待看目录不支持手动增删单本；标签权重不能批量设

---

## 十、谁是「用户」

本工程是**私人本地画廊**，真实库含成人内容。默认库路径 `F:\Hthings\BENZI`。
不要把这个库的内容复制、上传或外发。
