use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use rusqlite::types::Value;
use rusqlite::{params, Connection, OptionalExtension};
use tauri::AppHandle;
use walkdir::WalkDir;

use crate::archive;
use crate::db;
use crate::fsutil;
use crate::models::{ArchiveCard, ArchiveGroup, ArchivePage, ArchiveQuery, ScanReport};
use crate::paths;

/// 默认每页数量（与前端 48/页 的默认档一致）。
const DEFAULT_PAGE_SIZE: u32 = 48;
/// 单页最多返回多少条。
const MAX_PAGE_SIZE: u32 = 200;
/// 页码上限。OFFSET 随页码线性增长，过深的翻页等于全表扫描。
const MAX_QUERY_PAGE: u32 = 10_000;
/// 标签筛选最多参与多少个标签。
const MAX_TAG_FILTERS: usize = 200;
/// 搜索词最大长度，避免超长词展开成海量 trigram 查询。
const MAX_SEARCH_LEN: usize = 128;

pub fn scan_blocking(app: &AppHandle, root: &Path, library_id: &str) -> ScanReport {
    let started = Instant::now();
    let covers_dir = match paths::covers_dir(app, library_id) {
        Ok(dir) => dir,
        Err(error) => return scan_failure(error, started.elapsed().as_millis()),
    };
    scan_with_covers(root, &covers_dir)
}

/// 扫描入口：自行打开一条「只做连接配置、不跑迁移」的工作连接。
pub fn scan_with_covers(root: &Path, covers_dir: &Path) -> ScanReport {
    let started = Instant::now();
    match db::open_worker_connection(root) {
        Ok(connection) => scan_once(&connection, root, covers_dir, started),
        Err(error) => scan_failure(error, started.elapsed().as_millis()),
    }
}

/// 复用调用方已打开的连接扫描（桌面命令与 CLI 走这条路径，避免同进程开第二条连接）。
pub fn scan_connection(connection: &Connection, root: &Path, covers_dir: &Path) -> ScanReport {
    scan_once(connection, root, covers_dir, Instant::now())
}

fn scan_once(
    connection: &Connection,
    root: &Path,
    covers_dir: &Path,
    started: Instant,
) -> ScanReport {
    let mut errors = Vec::new();

    let mut files: Vec<PathBuf> = Vec::new();
    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| entry.file_name().to_string_lossy() != ".gallery")
    {
        match entry {
            Ok(entry) => {
                if entry.file_type().is_file() && archive::is_supported_archive(entry.path()) {
                    files.push(entry.into_path());
                }
            }
            Err(error) => errors.push(format!("遍历目录失败: {error}")),
        }
    }

    let found = files.len();
    let mut found_relative = HashSet::new();
    let mut added = 0usize;
    let mut updated = 0usize;

    for path in files {
        let relative = match fsutil::rel_to_string(root, &path) {
            Some(relative) => relative,
            None => {
                errors.push(format!("无法计算相对路径: {}", path.to_string_lossy()));
                continue;
            }
        };

        found_relative.insert(relative.clone());

        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => {
                errors.push(format!("无法读取文件信息 {}: {error}", path.to_string_lossy()));
                continue;
            }
        };

        let file_size = metadata.len() as i64;
        let file_mtime = fsutil::modified_ms(&metadata);

        let existing: Option<(i64, String, i64, i64, Option<String>, i64)> = match connection
            .query_row(
                "SELECT id, COALESCE(content_hash, ''), file_size, file_mtime, cover_key, missing
                 FROM archives WHERE rel_path = ?1",
                params![relative],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .optional()
        {
            Ok(value) => value,
            Err(error) => {
                errors.push(format!("查询索引失败 {}: {error}", path.to_string_lossy()));
                continue;
            }
        };

        if let Some((id, old_hash, old_size, old_mtime, old_cover, missing)) = existing {
            let cover_missing = old_cover
                .as_deref()
                .map(|key| !covers_dir.join(key).exists())
                .unwrap_or(true);

            if file_size != old_size || file_mtime != old_mtime {
                let content_hash = match fsutil::hash_file(&path) {
                    Ok(hash) => hash,
                    Err(error) => {
                        errors.push(format!("计算文件哈希失败 {}: {error}", path.to_string_lossy()));
                        continue;
                    }
                };

                let inspection = archive::inspect_archive(&path, &covers_dir, &content_hash);
                let file_name = file_name_of(&path);
                let extension = extension_of(&path);

                match db::update_archive_content(
                    connection,
                    id,
                    &relative,
                    &file_name,
                    &extension,
                    file_size,
                    file_mtime,
                    &content_hash,
                    inspection.page_count,
                    inspection.cover_key.as_deref(),
                    inspection.error.as_deref(),
                ) {
                    Ok(()) => updated += 1,
                    Err(error) => errors.push(format!("更新索引失败 {}: {error}", file_name)),
                }
            } else if cover_missing || missing != 0 {
                let content_hash = if old_hash.is_empty() {
                    match fsutil::hash_file(&path) {
                        Ok(hash) => hash,
                        Err(error) => {
                            errors.push(format!(
                                "计算文件哈希失败 {}: {error}",
                                path.to_string_lossy()
                            ));
                            continue;
                        }
                    }
                } else {
                    old_hash
                };

                let inspection = archive::inspect_archive(&path, &covers_dir, &content_hash);
                let file_name = file_name_of(&path);
                let extension = extension_of(&path);

                match db::update_archive_content(
                    connection,
                    id,
                    &relative,
                    &file_name,
                    &extension,
                    file_size,
                    file_mtime,
                    &content_hash,
                    inspection.page_count,
                    inspection.cover_key.as_deref(),
                    inspection.error.as_deref(),
                ) {
                    Ok(()) => updated += 1,
                    Err(error) => errors.push(format!("更新封面失败 {}: {error}", file_name)),
                }
            }

            continue;
        }

        let content_hash = match fsutil::hash_file(&path) {
            Ok(hash) => hash,
            Err(error) => {
                errors.push(format!("计算文件哈希失败 {}: {error}", path.to_string_lossy()));
                continue;
            }
        };

        let duplicate = match db::find_duplicate(connection, &content_hash) {
            Ok(value) => value,
            Err(error) => {
                errors.push(format!("检查重复失败 {}: {error}", path.to_string_lossy()));
                continue;
            }
        };

        let inspection = archive::inspect_archive(&path, &covers_dir, &content_hash);
        let file_name = file_name_of(&path);
        let extension = extension_of(&path);

        if let Some(id) = duplicate {
            match db::update_archive_content(
                connection,
                id,
                &relative,
                &file_name,
                &extension,
                file_size,
                file_mtime,
                &content_hash,
                inspection.page_count,
                inspection.cover_key.as_deref(),
                inspection.error.as_deref(),
            ) {
                Ok(()) => updated += 1,
                Err(error) => errors.push(format!("更新移动文件失败 {}: {error}", file_name)),
            }
        } else {
            match db::insert_archive(
                connection,
                &relative,
                &file_name,
                &extension,
                file_size,
                file_mtime,
                &content_hash,
                inspection.page_count,
                inspection.cover_key.as_deref(),
                inspection.error.as_deref(),
                fsutil::now_ms(),
                None,
            ) {
                Ok(_) => added += 1,
                Err(error) => errors.push(format!("写入索引失败 {}: {error}", file_name)),
            }
        }
    }

    let mut missing = 0usize;
    let mut to_mark: Vec<i64> = Vec::new();
    let existing_rows: Vec<(i64, String, i64)> = {
        let mut statement = match connection
            .prepare("SELECT id, rel_path, missing FROM archives WHERE deleted_at IS NULL")
        {
            Ok(statement) => statement,
            Err(error) => {
                errors.push(format!("读取缺失记录失败: {error}"));
                return ScanReport {
                    found,
                    added,
                    updated,
                    missing,
                    errors,
                    elapsed_ms: started.elapsed().as_millis(),
                    message: "扫描完成，但检查缺失文件失败".to_string(),
                };
            }
        };

        match statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
        {
            Ok(rows) => rows,
            Err(error) => {
                errors.push(format!("读取缺失记录失败: {error}"));
                Vec::new()
            }
        }
    };

    for (id, relative, was_missing) in existing_rows {
        if !found_relative.contains(&relative) && was_missing == 0 {
            to_mark.push(id);
        }
    }

    if !to_mark.is_empty() {
        match db::set_missing_many(connection, &to_mark) {
            Ok(changed) => missing = changed,
            Err(error) => errors.push(format!("标记缺失失败（{} 项）: {error}", to_mark.len())),
        }
    }

    let elapsed_ms = started.elapsed().as_millis();
    let message = format!(
        "扫描完成：发现 {found}，新增 {added}，更新 {updated}，缺失 {missing}，用时 {:.1}s",
        elapsed_ms as f64 / 1000.0
    );

    ScanReport {
        found,
        added,
        updated,
        missing,
        errors,
        elapsed_ms,
        message,
    }
}

fn scan_failure(message: String, elapsed_ms: u128) -> ScanReport {
    ScanReport {
        found: 0,
        added: 0,
        updated: 0,
        missing: 0,
        errors: vec![message.clone()],
        elapsed_ms,
        message: format!("扫描失败：{message}"),
    }
}

fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| "未命名".to_string())
}

fn extension_of(path: &Path) -> String {
    path.extension()
        .map(|value| value.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// 规整搜索词：去掉控制字符、合并连续空白、限长。
///
/// 搜索词会被拼成 FTS5 短语查询，控制字符（如 NUL）会让 MATCH 直接报错，
/// 而错误会冒泡成整个页面查询失败，所以先在这里清洗掉。
fn normalize_search(value: &str) -> Option<String> {
    let cleaned: String = value
        .chars()
        .filter(|ch| !ch.is_control())
        .take(MAX_SEARCH_LEN)
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        None
    } else {
        Some(collapsed)
    }
}

pub fn query_archives(
    connection: &Connection,
    root: &Path,
    covers_dir: &Path,
    query: &ArchiveQuery,
) -> Result<ArchivePage, String> {
    // page 直接决定 OFFSET，放任超大值会让 SQLite 扫描并丢弃海量行；
    // page_size 也被用来拼接 LIMIT，这里统一夹紧（与前端 12–200 的档位一致）。
    let page = query.page.unwrap_or(1).clamp(1, MAX_QUERY_PAGE);
    let page_size = query
        .page_size
        .unwrap_or(DEFAULT_PAGE_SIZE)
        .clamp(12, MAX_PAGE_SIZE);
    let offset = (page.saturating_sub(1) as i64) * page_size as i64;

    let sort = match query.sort.as_deref() {
        Some("file_mtime") => "a.file_mtime",
        Some("rating") => "a.rating",
        Some("file_size") => "a.file_size",
        Some("title") => "COALESCE(NULLIF(a.title, ''), a.file_name) COLLATE NOCASE",
        Some("page_count") => "a.page_count",
        Some("last_read_at") => "a.last_read_at",
        _ => "a.added_at",
    };

    let direction = if query
        .direction
        .as_deref()
        .map(|value| value.eq_ignore_ascii_case("asc"))
        .unwrap_or(false)
    {
        "ASC"
    } else {
        "DESC"
    };

    let mut conditions: Vec<String> = vec!["a.deleted_at IS NULL".to_string()];

    if let Some(collection_id) = query.collection_id {
        conditions.push(format!(
            "EXISTS (
                SELECT 1 FROM collection_archives ca
                WHERE ca.archive_id = a.id AND ca.collection_id = {collection_id}
            )"
        ));
    }

    if let Some(queue_id) = query.queue_id {
        conditions.push(format!(
            "EXISTS (
                SELECT 1 FROM queue_archives qa
                WHERE qa.archive_id = a.id AND qa.queue_id = {queue_id}
            )"
        ));
    }

    if let Some(ids) = query.archive_ids.as_ref() {
        let list = ids
            .iter()
            .filter(|id| **id > 0)
            .take(MAX_TAG_FILTERS)
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        if !list.is_empty() {
            conditions.push(format!("a.id IN ({list})"));
        }
    }

    let mut tag_ids: Vec<i64> = Vec::new();
    if let Some(ids) = query.tag_ids.as_ref() {
        for id in ids {
            if *id > 0 && !tag_ids.contains(id) {
                tag_ids.push(*id);
                // 标签筛选最终会拼成 IN (...) ，没有上限时一次请求就能生成
                // 触发 SQLite 表达式/长度上限的语句。
                if tag_ids.len() >= MAX_TAG_FILTERS {
                    break;
                }
            }
        }
    }

    // 合集（连载盒子）只在「纯浏览」时接管成员：把成员从网格拿掉，改由合集卡代表。
    // 一旦进入搜索 / 标签 / 收藏夹 / 待看 / 筛选 / 指定 id，就按普通画集显示——
    // 否则筛出来一片空白，用户会以为东西丢了。
    let plain_browse = query.collection_id.is_none()
        && query.queue_id.is_none()
        && query.archive_ids.is_none()
        && tag_ids.is_empty()
        && query.rating_buckets.as_ref().map_or(true, |v| v.is_empty())
        && query.read_statuses.as_ref().map_or(true, |v| v.is_empty())
        && query.missing_only != Some(true)
        && query.group.is_none()
        && query
            .search
            .as_ref()
            .map_or(true, |value| value.trim().is_empty());
    if plain_browse {
        conditions.push(
            "NOT EXISTS (
                SELECT 1 FROM series_items si
                JOIN series s ON s.id = si.series_id
                WHERE si.archive_id = a.id AND s.deleted_at IS NULL
            )"
            .to_string(),
        );
    }

    if !tag_ids.is_empty() {
        let list = tag_ids
            .iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let mode = query
            .tag_mode
            .as_deref()
            .unwrap_or("and")
            .to_ascii_lowercase();

        match mode.as_str() {
            "or" => conditions.push(format!(
                "a.id IN (
                    SELECT at.archive_id FROM archive_tags at
                    JOIN tags t ON t.id = at.tag_id AND t.deleted_at IS NULL
                    WHERE at.tag_id IN ({list})
                )"
            )),
            "exclude" => conditions.push(format!(
                "a.id NOT IN (
                    SELECT at.archive_id FROM archive_tags at
                    JOIN tags t ON t.id = at.tag_id AND t.deleted_at IS NULL
                    WHERE at.tag_id IN ({list})
                )"
            )),
            _ => conditions.push(format!(
                "a.id IN (
                    SELECT at.archive_id FROM archive_tags at
                    JOIN tags t ON t.id = at.tag_id AND t.deleted_at IS NULL
                    WHERE at.tag_id IN ({list})
                    GROUP BY at.archive_id
                    HAVING COUNT(DISTINCT at.tag_id) = {}
                )",
                tag_ids.len()
            )),
        }
    }

    let mut params: Vec<Value> = Vec::new();
    if let Some(buckets) = query.rating_buckets.as_ref() {
        let mut buckets: Vec<i64> = buckets
            .iter()
            .copied()
            .filter(|bucket| (0..=5).contains(bucket))
            .collect();
        buckets.sort_unstable();
        buckets.dedup();
        if !buckets.is_empty() {
            let list = buckets
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            conditions.push(format!(
                "(CASE WHEN a.rating <= 0 THEN 0 ELSE (a.rating + 19) / 20 END) IN ({list})"
            ));
        }
    }

    if let Some(statuses) = query.read_statuses.as_ref() {
        let has = |name: &str| {
            statuses
                .iter()
                .any(|value| value.eq_ignore_ascii_case(name))
        };
        let mut parts: Vec<&str> = Vec::new();
        if has("unread") {
            parts.push("a.last_page <= 0");
        }
        if has("reading") {
            parts.push("(a.last_page > 0 AND a.page_count > 0 AND a.last_page < a.page_count)");
        }
        if has("finished") {
            parts.push("(a.page_count > 0 AND a.last_page >= a.page_count)");
        }
        if !parts.is_empty() {
            conditions.push(format!("({})", parts.join(" OR ")));
        }
    }

    if query.missing_only.unwrap_or(false) {
        conditions.push("a.missing != 0".to_string());
    }

    if let Some(search) = query.search.as_deref().and_then(normalize_search) {
        // 3 字以上走 trigram 全文索引；1–2 字 trigram 无法命中，退回 LIKE。
        // 两条路径都覆盖 tag_text（v9 冗余列：标签原文名+译名），搜中文标签名也能找到画集。
        if search.chars().count() >= 3 {
            let escaped = search.replace('"', "\"\"");
            conditions.push(
                "a.id IN (SELECT rowid FROM archives_fts WHERE archives_fts MATCH ?)".to_string(),
            );
            params.push(Value::Text(format!("\"{escaped}\"")));
        } else {
            let like = format!("%{}%", escape_like(&search));
            conditions.push(
                "(COALESCE(a.title, '') LIKE ? ESCAPE '\\'
                  OR a.file_name LIKE ? ESCAPE '\\'
                  OR COALESCE(a.note, '') LIKE ? ESCAPE '\\'
                  OR a.tag_text LIKE ? ESCAPE '\\')"
                    .to_string(),
            );
            params.push(Value::Text(like.clone()));
            params.push(Value::Text(like.clone()));
            params.push(Value::Text(like.clone()));
            params.push(Value::Text(like));
        }
    }

    let filter = if conditions.is_empty() {
        "1=1".to_string()
    } else {
        conditions.join(" AND ")
    };

    let group_by_rating = query
        .group
        .as_deref()
        .map(|value| value.eq_ignore_ascii_case("rating"))
        .unwrap_or(false);
    let group_by_tag = query
        .group
        .as_deref()
        .map(|value| value.eq_ignore_ascii_case("tag"))
        .unwrap_or(false);
    let group_by_collection = query
        .group
        .as_deref()
        .map(|value| value.eq_ignore_ascii_case("collection"))
        .unwrap_or(false);
    let preferred_tag_order = if !group_by_tag {
        String::new()
    } else if !tag_ids.is_empty()
        && !query
            .tag_mode
            .as_deref()
            .map(|mode| mode.eq_ignore_ascii_case("exclude"))
            .unwrap_or(false)
    {
        // 用过滤后的 tag_ids（去重、去非法值、限长），与筛选条件保持同一口径。
        let ids = tag_ids
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        format!("CASE WHEN t.id IN ({ids}) THEN 0 ELSE 1 END, ")
    } else {
        String::new()
    };

    let tag_join = if group_by_tag {
        format!(
            r#"
            LEFT JOIN (
                SELECT archive_id, tag_id, label, category FROM (
                    SELECT at.archive_id,
                           t.id AS tag_id,
                           COALESCE(NULLIF(t.translated_name, ''), t.name) AS label,
                           t.category AS category,
                           ROW_NUMBER() OVER (
                               PARTITION BY at.archive_id
                               ORDER BY {preferred_tag_order}
                                        COALESCE(NULLIF(t.translated_name, ''), t.name) COLLATE NOCASE,
                                        t.id
                           ) AS rn
                    FROM archive_tags at
                    JOIN tags t ON t.id = at.tag_id AND t.deleted_at IS NULL
                )
                WHERE rn = 1
            ) pt ON pt.archive_id = a.id
            "#
        )
    } else {
        String::new()
    };

    let collection_join = if group_by_collection {
        // When browsing one collection, its members should group under it even
        // if they also belong to an earlier-ordered collection.
        let preferred_collection_order = query
            .collection_id
            .map(|id| format!("CASE WHEN c.id = {id} THEN 0 ELSE 1 END, "))
            .unwrap_or_default();
        format!(
            r#"
            LEFT JOIN (
                SELECT archive_id, collection_id, label, is_default, sort_order FROM (
                    SELECT ca.archive_id,
                           c.id AS collection_id,
                           c.name AS label,
                           c.is_default AS is_default,
                           c.sort_order AS sort_order,
                           ROW_NUMBER() OVER (
                               PARTITION BY ca.archive_id
                               ORDER BY {preferred_collection_order}
                                        c.is_default DESC,
                                        c.sort_order ASC,
                                        c.created_at ASC,
                                        c.id ASC
                           ) AS rn
                    FROM collection_archives ca
                    JOIN collections c ON c.id = ca.collection_id AND c.deleted_at IS NULL
                )
                WHERE rn = 1
            ) pc ON pc.archive_id = a.id
            "#
        )
    } else {
        String::new()
    };

    let groups = if group_by_rating {
        let mut statement = connection
            .prepare(&format!(
                "SELECT CASE WHEN a.rating <= 0 THEN 0 ELSE (a.rating + 19) / 20 END AS bucket,
                        COUNT(*)
                 FROM archives a
                 WHERE {filter}
                 GROUP BY bucket
                 ORDER BY bucket {direction}"
            ))
            .map_err(|error| format!("准备分组查询失败: {error}"))?;

        let rows = statement
            .query_map(
                rusqlite::params_from_iter(params.iter().cloned()),
                |row| {
                    Ok(ArchiveGroup {
                        bucket: row.get(0)?,
                        count: row.get(1)?,
                        label: None,
                        category: None,
                    })
                },
            )
            .map_err(|error| format!("查询分组失败: {error}"))?;

        Some(
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|error| format!("读取评分分组失败: {error}"))?,
        )
    } else if group_by_tag {
        let mut statement = connection
            .prepare(&format!(
                "SELECT COALESCE(pt.tag_id, 0) AS bucket,
                        COUNT(*),
                        MAX(pt.label),
                        MAX(pt.category)
                 FROM archives a
                 {tag_join}
                 WHERE {filter}
                 GROUP BY COALESCE(pt.tag_id, 0)
                 ORDER BY (COALESCE(pt.tag_id, 0) = 0), MAX(pt.label) COLLATE NOCASE"
            ))
            .map_err(|error| format!("准备标签分组查询失败: {error}"))?;

        let rows = statement
            .query_map(
                rusqlite::params_from_iter(params.iter().cloned()),
                |row| {
                    Ok(ArchiveGroup {
                        bucket: row.get(0)?,
                        count: row.get(1)?,
                        label: row.get(2)?,
                        category: row.get(3)?,
                    })
                },
            )
            .map_err(|error| format!("查询标签分组失败: {error}"))?;

        Some(
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|error| format!("读取标签分组失败: {error}"))?,
        )
    } else if group_by_collection {
        let mut statement = connection
            .prepare(&format!(
                "SELECT COALESCE(pc.collection_id, 0) AS bucket,
                        COUNT(*),
                        MAX(pc.label)
                 FROM archives a
                 {collection_join}
                 WHERE {filter}
                 GROUP BY COALESCE(pc.collection_id, 0)
                 ORDER BY (COALESCE(pc.collection_id, 0) = 0),
                          MAX(pc.is_default) DESC,
                          MIN(pc.sort_order),
                          MAX(pc.label) COLLATE NOCASE"
            ))
            .map_err(|error| format!("准备收藏夹分组查询失败: {error}"))?;

        let rows = statement
            .query_map(
                rusqlite::params_from_iter(params.iter().cloned()),
                |row| {
                    Ok(ArchiveGroup {
                        bucket: row.get(0)?,
                        count: row.get(1)?,
                        label: row.get(2)?,
                        category: None,
                    })
                },
            )
            .map_err(|error| format!("查询收藏夹分组失败: {error}"))?;

        Some(
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|error| format!("读取收藏夹分组失败: {error}"))?,
        )
    } else {
        None
    };

    let total: i64 = connection
        .query_row(
            &format!("SELECT COUNT(*) FROM archives a WHERE {filter}"),
            rusqlite::params_from_iter(params.iter().cloned()),
            |row| row.get(0),
        )
        .map_err(|error| format!("统计画集数量失败: {error}"))?;

    params.push(Value::Integer(page_size as i64));
    params.push(Value::Integer(offset));

    let group_order = if group_by_rating {
        format!("a.rating {direction}, ")
    } else if group_by_tag {
        "(COALESCE(pt.tag_id, 0) = 0), pt.label COLLATE NOCASE, ".to_string()
    } else if group_by_collection {
        "(COALESCE(pc.collection_id, 0) = 0), pc.is_default DESC, pc.sort_order, pc.label COLLATE NOCASE, "
            .to_string()
    } else {
        String::new()
    };

    let group_columns = if group_by_tag {
        "COALESCE(pt.tag_id, 0), pt.label, pt.category"
    } else if group_by_collection {
        "COALESCE(pc.collection_id, 0), pc.label, NULL"
    } else {
        "NULL, NULL, NULL"
    };

    // Unread archives always sink to the bottom, whatever the direction.
    let null_order = if query.sort.as_deref() == Some("last_read_at") {
        "(a.last_read_at IS NULL), "
    } else {
        ""
    };

    // 队列视图按队列顺序（queue_archives.added_at ASC）排——「造队列」的顺序
    // 就是阅读顺序，用户的其它排序键在队列视图里让位。所有行都是队列成员
    // （WHERE 已用 EXISTS 过滤），子查询不会返回 NULL。
    let queue_order = query
        .queue_id
        .map(|qid| {
            format!(
                "(SELECT qa.added_at FROM queue_archives qa \
                  WHERE qa.queue_id = {qid} AND qa.archive_id = a.id) ASC, "
            )
        })
        .unwrap_or_default();

    let sql = format!(
        "SELECT a.id,
                COALESCE(NULLIF(a.title, ''), a.file_name),
                a.file_name,
                a.ext,
                a.file_size,
                a.rating,
                a.page_count,
                a.added_at,
                a.file_mtime,
                a.cover_key,
                a.rel_path,
                a.missing,
                a.error,
                a.note,
                {group_columns},
                a.last_read_at,
                a.last_page,
                a.read_count
         FROM archives a
         {tag_join}{collection_join}
         WHERE {filter}
         ORDER BY {queue_order}{group_order}{null_order}{sort} {direction}, a.id DESC
         LIMIT ? OFFSET ?"
    );

    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| format!("准备查询失败: {error}"))?;

    let rows = statement
        .query_map(
            rusqlite::params_from_iter(params.iter().cloned()),
            |row| {
                let cover_key: Option<String> = row.get(9)?;
                let relative: String = row.get(10)?;
                let missing: i64 = row.get(11)?;
                let file_path = fsutil::join_rel(root, &relative)
                    .map(|path| path.to_string_lossy().to_string())
                    .unwrap_or_default();
                let cover_path = cover_key.map(|key| {
                    covers_dir
                        .join(key)
                        .to_string_lossy()
                        .to_string()
                });

                Ok(ArchiveCard {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    file_name: row.get(2)?,
                    ext: row.get(3)?,
                    file_size: row.get(4)?,
                    rating: row.get(5)?,
                    page_count: row.get(6)?,
                    added_at: row.get(7)?,
                    file_mtime: row.get(8)?,
                    cover_path,
                    file_path,
                    note: row.get(13)?,
                    missing: missing != 0,
                    error: row.get(12)?,
                    group_bucket: row.get(14)?,
                    group_label: row.get(15)?,
                    group_category: row.get(16)?,
                    last_read_at: row.get(17)?,
                    last_page: row.get(18)?,
                    read_count: row.get(19)?,
                })
            },
        )
        .map_err(|error| format!("查询画集失败: {error}"))?;

    let items: Vec<ArchiveCard> = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("读取画集行失败: {error}"))?;

    let series = if plain_browse && page == 1 {
        Some(crate::series::list_series(connection, covers_dir)?)
    } else {
        None
    };

    Ok(ArchivePage {
        items,
        total,
        page,
        page_size,
        groups,
        series,
    })
}
