# 画匣 · 本地画廊

> 把散落的图片压缩包收进一座本地画廊：封面墙、评分、标签、合集，外加一个顺手的阅读器。
> **完全离线 —— 不联网、不注册、不上传任何文件。**

一个 Windows 桌面应用（Tauri 2 + React 19 + Rust），把 ZIP / CBZ / 7Z / RAR 里的图片整理成可浏览的库：自动抽封面、建索引、打分、挂标签、组合集，双击封面直接进入内置阅读器。

---

## 功能

**导入与库**
- 库目录任意选择；压缩包**复制入库**（源文件不动），重复导入自动去重
- 支持 ZIP / CBZ / 7Z / RAR，也支持文件夹；重新扫描、封面缓存、缺失检测
- 一条画集 = 一个压缩包；包内多图 = 多页

**浏览**
- 封面网格，滚动 / 分页两种模式，一屏内右键即可切换
- 按评分 / 标签 / 收藏夹分组，阅读进度徽标，筛选状态按库记忆
- 侧栏三档自适应：完整 → 收起行内按钮 → 只留图标（最低 62px）
- 收藏夹自定义图标：五角星 / 爱心 / 圆形 / 正方形 × 8 色纯色块

**阅读器**
- 缩放（`+` `-` `0` / Ctrl+滚轮）、双页、沉浸模式（连标题栏一起收）、缩略图栏
- GIF 播放（动图播完再翻页）、幻灯播放（`Ctrl+1~9` 每 N 秒一页）
- 书签、书签列表、跨本连续阅读（读到底自动接下一本）
- 复制当前页到剪贴板、在资源管理器中定位、一键设为桌面壁纸
- 老板键：`Ctrl+H` 立刻藏窗，`Ctrl+Shift+H` 全局呼回
- 鼠标后退侧键：沉浸态先退沉浸，再按退出阅读

**整理**
- 百分制评分、备注、标签（分类 / 翻译名 / 权重）
- 标签中心：层级分类、合并、回收站、标签导图、**一键保存全部修改**
- 合集（连载盒子）：按集数顺序排队，可改每话名字、设封面、封面墙浏览
- 待看目录（随机排队 + 进度）、散图收藏夹、统一回收站

**检索**
- FTS5 全文搜索（标题 / 文件名 / 备注 / 标签），标签 AND / OR / 排除
- 评分区间、阅读状态、文件缺失组合筛选

**自定义**
- 16 个阅读器动作可改键（设置 → 快捷键，冲突检测、恢复默认）
- 三处右键菜单：阅读器 / 画集卡片 / 画廊空白处
- 应用内「设置 → 新手教程」随时可查

---

## 下载安装

Windows 10 / 11（64 位）。到 [Releases](../../releases) 下载最新 `画匣_<版本>_x64-setup.exe`，双击安装（装到当前用户，无需管理员权限）。

首次启动需要**选一个库目录**——建议专门建一个，例如 `D:\画匣库`。库里存的是压缩包原件与 `.gallery`（索引、封面缓存、自动备份），整个目录拷走即可迁移。

详细图文说明见 [`docs/使用说明.html`](docs/使用说明.html)（浏览器打开，可打印）或 [`docs/使用说明.md`](docs/使用说明.md)。

---

## 从源码构建

需要 Node.js 20+、Rust 1.80+、Windows 上的 MSVC 工具链。

```bash
npm install
npm run tauri dev        # 开发模式启动
npm run check            # TypeScript 类型检查
cargo test --manifest-path src-tauri/Cargo.toml --lib   # 后端测试
npm run build:bundle     # 打 NSIS 安装包
```

产物：`src-tauri/target/release/bundle/nsis/画匣_<版本>_x64-setup.exe`。
首次打包需要 Tauri 下载 NSIS 工具并缓存到 `%LOCALAPPDATA%\tauri\NSIS`；网络不通时可指定镜像：

```bash
TAURI_BUNDLER_TOOLS_GITHUB_MIRROR="https://ghproxy.net/https://github.com" npm run build:bundle
```

## 发布流程

```bash
node scripts/release.mjs            # 打补丁版本（+1）
node scripts/release.mjs --minor    # 次版本 +1（正式发版，LTO 瘦身，构建较慢）
node scripts/release.mjs --fast     # 热修迭代（关 LTO，构建快，体积大）
```

脚本会：跑后端测试 → 构建前端与 Rust → 打 NSIS 包 → 安装并重启 → 提交版本号。

## 目录结构

```
src/                  前端界面（React + TS）
src-tauri/src/        Rust 后端（导入、扫描、查询、SQLite、阅读器容器）
src-tauri/icons/      应用图标（app-icon.png 为源图，其余由 npx tauri icon 生成）
scripts/release.mjs   发版脚本
docs/使用说明.*        面向使用者的说明
```

## 技术栈

Tauri 2 · React 19 · TypeScript · Vite 8；Rust 后端为 rusqlite（SQLite WAL + FTS5）、zip、image、blake3。数据库升级前自动备份，版本号内建迁移。

## 已知限制

- 仅 Windows 打包验证过（Tauri 跨平台可用，但未做 macOS / Linux 适配与测试）
- 暂无内置的多语言界面（简体中文）
- 大于约 64MB 的单页图片会在阅读器中拒绝加载，避免整块读入内存

## 贡献

欢迎 Issue 与 PR。提 PR 前请跑 `npm run check` 与后端测试；涉及数据库结构的改动请附带迁移与备份验证。

## 许可

MIT，见 [LICENSE](LICENSE)。
