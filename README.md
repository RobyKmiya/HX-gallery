# 画匣 · 本地画廊

本地离线图集画廊：把 ZIP / CBZ 压缩包统一导入库目录，总览封面、查看详情、评分与收藏，并支持库内快速检索。全程不联网。

## 功能状态

- 库与导入：库目录切换、ZIP/CBZ/文件夹导入、去重、重新扫描、封面缓存、缺失检测
- 浏览：网格总览、排序、滚动/分页、按评分/标签/收藏夹分组、阅读进度徽标
- 详情：评分、备注、标签、收藏夹、阅读进度、内置阅读器（双页、书签、缩略图）
- 组织：标签中心（改名/翻译/分类层级/合并/回收站）、标签导图、收藏夹管理、统一回收站
- 检索：FTS5 全文搜索、标签 AND/OR/排除、评分区间 / 阅读状态 / 文件缺失组合筛选，筛选状态按库记忆
- 发布：`1.0.0` Windows NSIS 安装包，附带 `galleryctl` 命令行接口

## 技术栈

Tauri 2 + React 19 + TypeScript + Vite 8；Rust 后端使用 rusqlite（SQLite WAL）、zip、image、blake3。

## 开发

```bash
npm install
npm run tauri dev      # 启动桌面应用
npm run check          # TypeScript 类型检查
npm run build          # 前端构建
cargo test --manifest-path src-tauri/Cargo.toml --lib   # 后端集成测试
```

## 发布与打包

```bash
npm run build:bundle
```

产物：`src-tauri/target/release/bundle/nsis/画匣_<版本>_x64-setup.exe`（当前用户安装，简体中文）。安装包会附带同目录的 `galleryctl.exe`（AI/脚本接口，说明见 [docs/AGENT_API.md](docs/AGENT_API.md)）。

首次打包需要 Tauri 下载 NSIS 工具并缓存到 `%LOCALAPPDATA%\tauri\NSIS`。若 GitHub Release 资源不可达，可临时指定镜像：

```bash
TAURI_BUNDLER_TOOLS_GITHUB_MIRROR="https://ghproxy.net/https://github.com" npm run build:bundle
```

## 目录

- `src/` 前端界面与 API 封装
- `src-tauri/src/` Rust 后端（导入、扫描、查询、数据库）
- `src-tauri/icons/` 应用图标（`app-icon.png` 为源图，其余由 `npx tauri icon` 生成）
- `docs/WORKLOG.md` 工作日志总览；`docs/logs/` 按日期记录的设计与验证细节

开发过程、关键决策与遗留问题见 [docs/WORKLOG.md](docs/WORKLOG.md)。
