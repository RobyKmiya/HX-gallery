# AI Agent 接口：galleryctl

`galleryctl` 是画廊自带的本地 JSON 命令行接口，供 AI agent 分批完成标签翻译、标签合并、回收站管理、收藏夹管理和分页搜索。它直接读写同一个 SQLite 库，不需要启动桌面应用。

## 调用方式

```bash
# 调试构建（本仓库）
src-tauri/target/debug/galleryctl.exe --library "F:/Hthings/BENZI" stats

# 或用环境变量
ARCHIVE_GALLERY_LIBRARY="F:/Hthings/BENZI" galleryctl stats
```

- 库路径必须是已经初始化过的画廊根目录（含 `.gallery/library.db`）。
- `--library` / `ARCHIVE_GALLERY_LIBRARY` 全局可用，子命令前后都能写。
- `--compact` 全局可用，输出单行 JSON，适合程序解析。

## 输出约定

成功（stdout，退出码 0）：

```json
{ "ok": true, "data": { } }
```

失败（stderr，退出码 1）：

```json
{ "ok": false, "error": "中文错误说明" }
```

所有命令都是非交互的，不会弹窗。分页命令使用游标，不需要一次性读全量。

## 命令速查

### 库与统计

```bash
galleryctl init --library <dir>                  # 初始化库（保留已有文件）
galleryctl stats                                 # archives/totalSize/missing/tags/collections
galleryctl scan [--covers-dir <dir>]             # 重新扫描库根目录
galleryctl import --paths <文件或目录> [--paths <...>] [--collection-id <id>]
```

`import` 的源文件如果已经在库根目录内，只做哈希建索引，**不会复制**；库外文件才会复制进来。`--paths` 建议每个路径重复写一次。

### 搜索（分页）

```bash
galleryctl search --query "关键词" --page 1 --limit 48 \
  --sort rating --direction desc \
  --collection-id 2 --tag-ids 3,7 --tag-mode and \
  --rating-buckets 4,5 --read-status reading,unread --missing
```

- `--query`：标题 / 文件名 / 备注。≥3 字符走 FTS5 trigram 短语匹配；1–2 字符走 LIKE 包含匹配。
- `--limit`：默认 48，实际会被限制在 12–200。
- `--sort`：`added_at`（默认）、`last_read_at`、`file_mtime`、`rating`、`file_size`、`title`、`page_count`。按 `last_read_at` 排序时未读画集始终排在最后（`asc` / `desc` 都一样）。
- `--direction`：`asc` / `desc`（默认 `desc`）。
- `--tag-mode`：`and`（默认，同时含）/ `or`（任一）/ `exclude`（排除）。
- `--rating-buckets`：评分桶，`0` 未评分，`1`–`5` 对应 1–20 / 21–40 / 41–60 / 61–80 / 81–100。多选为任一匹配。
- `--read-status`：`unread`（未读）/ `reading`（阅读中）/ `finished`（已读完），多选为任一匹配。
- `--missing`：只看文件缺失的画集。
- 所有维度之间取交集，可与标签、收藏夹、搜索任意叠加。
- 返回：`{ items, total, page, pageSize }`。翻页用 `--page` 递增。每个 item 含 `lastReadAt`（毫秒时间戳或 `null`）与 `lastPage`（1 基页码，`0` 表示未读）。

### 标签

```bash
# 游标分页列出标签
galleryctl tags list --status active --limit 100
galleryctl tags list --status untranslated --limit 100 --after 120

# 批量写翻译（文件或 stdin）
galleryctl tags translate --input translations.json
cat translations.json | galleryctl tags translate --input -

# 单项编辑 / 显示名 / 分类
galleryctl tags update --id 12 --translated-name "中文译名"
galleryctl tags update --id 12 --clear-translation
galleryctl tags update --id 12 --name "新的显示名" --category "题材"

# 合并与回收站
galleryctl tags merge --source 12,15 --target 9
galleryctl tags delete --ids 12,15            # 软删除，进入回收站
galleryctl tags delete --ids 12,15 --purge    # 仅回收站中的标签可彻底删除
galleryctl tags restore --ids 12,15

# 画集打标签 / 去标签
galleryctl tags attach --archive 3 --tags "warm,cool"
galleryctl tags detach --archive 3 --tag-ids 2,4

# 导图与统计
galleryctl tags graph --min-weight 2 --limit 300
galleryctl tags stats
```

要点：

- `tags list` 返回 `{ status, matched, returned, nextAfter, tags }`。当 `returned` 等于 `limit` 时继续用 `--after <nextAfter>`，直到 `nextAfter` 为 `null`。
- 每个标签有 `number`（首次创建时按导入顺序分配，永久不变）和 `originalName`（首次导入的原始名，不可修改）。`name` 是显示名，`translatedName` 是中文译名。
- `tags translate` 输入：

  ```json
  [
    { "id": 12, "translatedName": "温暖", "source": "ai" },
    { "id": 15, "translatedName": "冷色" }
  ]
  ```

  也可以包一层：`{ "translations": [...] }`。`source` 默认 `ai`，返回 `{ applied, ids }`。
- `merge` 会把来源标签的关联迁移到目标，然后把来源软删除并记录 `mergedInto`。合并产生的来源标签**不能单独恢复**；之后用来源名字 `attach` 会自动落到目标标签上。
- `purge` 不可恢复，且只对回收站中的标签生效。
- `attach` 的 `--tags` 传标签名（逗号分隔），不是 id；`detach` 的 `--tag-ids` 才是 id。
- `tags graph` 返回 `{ nodes, edges }`，`edges` 为 `{ source, target, weight }`。`limit` 限制边数（1–2000，默认 300）。

### 收藏夹

```bash
galleryctl collections list
galleryctl collections create --name "稍后阅读"
galleryctl collections rename --id 2 --name "新名字"
galleryctl collections delete --id 2                 # 默认收藏夹不可删除；删除即移入回收站
galleryctl collections trash                         # 列出回收站中的收藏夹
galleryctl collections restore --id 2                # 从回收站恢复，画集关联保留
galleryctl collections purge --id 2                  # 仅回收站中的收藏夹可彻底删除
galleryctl collections reorder --ids 1,3,2
galleryctl collections cover --id 2 --archive-id 17
galleryctl collections cover --id 2 --clear
galleryctl collections add --id 2 --archives 17,21,25
galleryctl collections remove --id 2 --archives 17
```

- `cover` 不传 `--archive-id` 也不传 `--clear` 时等同于清除封面。
- `add` / `remove` 的 `--archives` 用逗号分隔，返回成功与失败明细。
- 删除是软删除，保留画集关联；`purge` 不可恢复。回收站中的名字仍占用唯一名称，需先恢复或彻底删除才能重新创建同名收藏夹。

### 备注

```bash
galleryctl archives note --id 17 --text "系列第三本，画质很好"
galleryctl archives note --id 17 --clear
```

备注会参与 `search --query` 的文本检索。

## 推荐的 AI 工作流

### 1. 分批翻译

1. `tags list --status untranslated --limit 100`。
2. 把返回的 `tags` 交给模型翻译，生成 `[{ "id": ..., "translatedName": ... }]`。
3. `tags translate --input -` 写回。
4. 若 `nextAfter` 不为 `null`，重复 1–3，直到全部翻译完成。
5. `tags stats` 检查 `untranslated` 是否为 0。

### 2. 迭代清理同义标签

1. `tags list --status active --limit 200` 或 `--query` 缩小范围。
2. 选出同义组，指定一个目标，执行 `tags merge --source ... --target ...`。
3. 每批完成后重新 `tags stats`，避免一次处理过多。

### 3. 分页检查库存

```bash
galleryctl search --query "" --page 1 --limit 100 --sort added_at
# 用返回的 total / pageSize 计算页数，再逐页读取
```

## 注意事项

- 这是私人本地库，命令只在本地读写，不联网。
- 不要删除库根目录或 `.gallery`；`tags delete --purge` 与 `collections purge` 不可恢复。
- 桌面应用与 CLI 可以同时使用，SQLite 已开启 WAL；但不要用其他工具长期持有写锁。
- 库切换只影响设置文件；CLI 始终以 `--library` / 环境变量为准，执行 destructive 命令前先 `stats` 确认目标库。
