//! 合集（连载盒子）：把多本画集归到一处，主网格只留一张卡，成员按集数排列。
//!
//! 设计原则：
//! - **不复制文件、不复制画集**，`series_items` 只是"哪本、第几话"的关系表；
//! - 软删除：`series.deleted_at` 非空即视为已拆散，成员自动回到主网格；
//! - 一本画集同一时间只能在一个合集里（`archive_id` 上有唯一约束兜底）。

use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

use crate::fsutil;
use crate::models::{SeriesItem, SeriesSummary};

/// 全部合集（含成员），按创建时间新的在前。
pub fn list_series(
    connection: &Connection,
    covers_dir: &Path,
) -> Result<Vec<SeriesSummary>, String> {
    let mut statement = connection
        .prepare(
            "SELECT s.id, s.name, s.note, s.cover_archive_id,
                    (SELECT COUNT(*) FROM series_items si
                      JOIN archives a ON a.id = si.archive_id AND a.deleted_at IS NULL
                     WHERE si.series_id = s.id)
             FROM series s
             WHERE s.deleted_at IS NULL
             ORDER BY s.created_at DESC, s.id DESC",
        )
        .map_err(|e| format!("准备合集查询失败: {e}"))?;

    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .map_err(|e| format!("查询合集失败: {e}"))?;

    let mut out = Vec::new();
    for row in rows {
        let (id, name, note, cover_archive_id, item_count) =
            row.map_err(|e| format!("读取合集行失败: {e}"))?;
        let items = list_items(connection, covers_dir, id)?;
        out.push(SeriesSummary {
            id,
            name,
            note,
            cover_archive_id,
            item_count,
            items,
        });
    }
    Ok(out)
}

/// 某个合集的成员，按 position 升序。
pub fn list_items(
    connection: &Connection,
    covers_dir: &Path,
    series_id: i64,
) -> Result<Vec<SeriesItem>, String> {
    let mut statement = connection
        .prepare(
            "SELECT si.archive_id, si.position, si.label,
                    COALESCE(NULLIF(a.title, ''), a.file_name),
                    a.cover_key, a.page_count, a.missing
             FROM series_items si
             JOIN archives a ON a.id = si.archive_id
             WHERE si.series_id = ?1 AND a.deleted_at IS NULL
             ORDER BY si.position ASC, si.archive_id ASC",
        )
        .map_err(|e| format!("准备合集成员查询失败: {e}"))?;

    let rows = statement
        .query_map(params![series_id], |row| {
            let cover_key: Option<String> = row.get(4)?;
            Ok(SeriesItem {
                archive_id: row.get(0)?,
                position: row.get(1)?,
                label: row.get(2)?,
                title: row.get(3)?,
                cover_path: cover_key
                    .map(|key| covers_dir.join(key).to_string_lossy().to_string()),
                page_count: row.get(5)?,
                missing: row.get::<_, i64>(6)? != 0,
            })
        })
        .map_err(|e| format!("查询合集成员失败: {e}"))?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取合集成员失败: {e}"))
}

/// 新建合集。返回新 id；任何一本已属于别的合集就整体拒绝，不做部分成功。
pub fn create_series(
    connection: &Connection,
    name: &str,
    archive_ids: &[i64],
) -> Result<i64, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("合集名字不能为空".to_string());
    }
    if archive_ids.is_empty() {
        return Err("至少要选一本画集".to_string());
    }

    let transaction = crate::db::begin_write(connection)?;
    for archive_id in archive_ids {
        let exists: Option<i64> = transaction
            .query_row(
                "SELECT id FROM archives WHERE id = ?1 AND deleted_at IS NULL",
                params![archive_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("查询画集失败: {e}"))?;
        if exists.is_none() {
            return Err(format!("画集 {archive_id} 不存在或已删除"));
        }
        let occupied = series_of_archive_in(&transaction, *archive_id)?;
        if occupied.is_some() {
            return Err("所选画集里有已在其他合集中的，请先拆散".to_string());
        }
    }

    transaction
        .execute(
            "INSERT INTO series(name, note, created_at) VALUES (?1, NULL, ?2)",
            params![name, fsutil::now_ms()],
        )
        .map_err(|e| format!("创建合集失败: {e}"))?;
    let series_id = transaction.last_insert_rowid();

    for (index, archive_id) in archive_ids.iter().enumerate() {
        transaction
            .execute(
                "INSERT OR IGNORE INTO series_items(series_id, archive_id, position, label)
                 VALUES (?1, ?2, ?3, NULL)",
                params![series_id, archive_id, index as i64],
            )
            .map_err(|e| format!("加入合集失败: {e}"))?;
    }

    // 默认用第一本当封面。
    transaction
        .execute(
            "UPDATE series SET cover_archive_id = ?1 WHERE id = ?2",
            params![archive_ids[0], series_id],
        )
        .map_err(|e| format!("设置合集封面失败: {e}"))?;

    transaction
        .commit()
        .map_err(|e| format!("提交合集失败: {e}"))?;
    Ok(series_id)
}

/// 改名字 / 简介。
pub fn update_series(
    connection: &Connection,
    series_id: i64,
    name: &str,
    note: Option<&str>,
) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("合集名字不能为空".to_string());
    }
    let changed = connection
        .execute(
            "UPDATE series SET name = ?1, note = ?2 WHERE id = ?3 AND deleted_at IS NULL",
            params![name, note, series_id],
        )
        .map_err(|e| format!("修改合集失败: {e}"))?;
    if changed == 0 {
        return Err("合集不存在".to_string());
    }
    Ok(())
}

/// 拆散：软删除合集，成员回到主网格（`series_items` 保留，方便日后恢复）。
pub fn dissolve_series(connection: &Connection, series_id: i64) -> Result<(), String> {
    let changed = connection
        .execute(
            "UPDATE series SET deleted_at = ?1 WHERE id = ?2 AND deleted_at IS NULL",
            params![fsutil::now_ms(), series_id],
        )
        .map_err(|e| format!("拆散合集失败: {e}"))?;
    if changed == 0 {
        return Err("合集不存在".to_string());
    }
    Ok(())
}

/// 指定封面（传 None 表示回到「按顺序第一本」）。
pub fn set_cover(
    connection: &Connection,
    series_id: i64,
    archive_id: Option<i64>,
) -> Result<(), String> {
    connection
        .execute(
            "UPDATE series SET cover_archive_id = ?1 WHERE id = ?2 AND deleted_at IS NULL",
            params![archive_id, series_id],
        )
        .map_err(|e| format!("设置合集封面失败: {e}"))?;
    Ok(())
}

/// 按给定顺序重排（没提到的成员保持原顺序接在后面）。
pub fn reorder(connection: &Connection, series_id: i64, ordered: &[i64]) -> Result<(), String> {
    let transaction = crate::db::begin_write(connection)?;
    let mut position = 0i64;
    for archive_id in ordered {
        let changed = transaction
            .execute(
                "UPDATE series_items SET position = ?1
                 WHERE series_id = ?2 AND archive_id = ?3",
                params![position, series_id, archive_id],
            )
            .map_err(|e| format!("调整顺序失败: {e}"))?;
        if changed > 0 {
            position += 1;
        }
    }
    transaction
        .commit()
        .map_err(|e| format!("提交排序失败: {e}"))?;
    Ok(())
}

/// 改某一本的集数标签（「第一话」「番外」都行）；传 None 清空。
pub fn set_label(
    connection: &Connection,
    series_id: i64,
    archive_id: i64,
    label: Option<&str>,
) -> Result<(), String> {
    let label = label.map(str::trim).filter(|value| !value.is_empty());
    let changed = connection
        .execute(
            "UPDATE series_items SET label = ?1 WHERE series_id = ?2 AND archive_id = ?3",
            params![label, series_id, archive_id],
        )
        .map_err(|e| format!("修改集数失败: {e}"))?;
    if changed == 0 {
        return Err("这本画集不在该合集中".to_string());
    }
    Ok(())
}

/// 追加成员（已属于别的合集会被跳过，不报错）。
pub fn add_archives(
    connection: &Connection,
    series_id: i64,
    archive_ids: &[i64],
) -> Result<usize, String> {
    let transaction = crate::db::begin_write(connection)?;
    let next: i64 = transaction
        .query_row(
            "SELECT COALESCE(MAX(position) + 1, 0) FROM series_items WHERE series_id = ?1",
            params![series_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("读取合集顺序失败: {e}"))?;

    let mut added = 0usize;
    let mut position = next;
    for archive_id in archive_ids {
        if series_of_archive_in(&transaction, *archive_id)?.is_some() {
            continue;
        }
        let changed = transaction
            .execute(
                "INSERT OR IGNORE INTO series_items(series_id, archive_id, position, label)
                 VALUES (?1, ?2, ?3, NULL)",
                params![series_id, archive_id, position],
            )
            .map_err(|e| format!("追加合集成员失败: {e}"))?;
        if changed > 0 {
            added += 1;
            position += 1;
        }
    }
    transaction
        .commit()
        .map_err(|e| format!("提交合集成员失败: {e}"))?;
    Ok(added)
}

/// 移出某一本（合集只剩 1 本时自动拆散，避免留下空壳）。
pub fn remove_archive(
    connection: &Connection,
    series_id: i64,
    archive_id: i64,
) -> Result<(), String> {
    let transaction = crate::db::begin_write(connection)?;
    transaction
        .execute(
            "DELETE FROM series_items WHERE series_id = ?1 AND archive_id = ?2",
            params![series_id, archive_id],
        )
        .map_err(|e| format!("移出合集失败: {e}"))?;
    let remaining: i64 = transaction
        .query_row(
            "SELECT COUNT(*) FROM series_items WHERE series_id = ?1",
            params![series_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("统计合集成员失败: {e}"))?;
    if remaining <= 1 {
        transaction
            .execute(
                "UPDATE series SET deleted_at = ?1 WHERE id = ?2 AND deleted_at IS NULL",
                params![fsutil::now_ms(), series_id],
            )
            .map_err(|e| format!("拆散合集失败: {e}"))?;
    }
    transaction
        .commit()
        .map_err(|e| format!("提交成员移除失败: {e}"))?;
    Ok(())
}

/// 这本画集属于哪个（未拆散的）合集。
pub fn series_of_archive(connection: &Connection, archive_id: i64) -> Result<Option<i64>, String> {
    series_of_archive_in(connection, archive_id)
}

fn series_of_archive_in(connection: &Connection, archive_id: i64) -> Result<Option<i64>, String> {
    connection
        .query_row(
            "SELECT si.series_id
             FROM series_items si
             JOIN series s ON s.id = si.series_id AND s.deleted_at IS NULL
             WHERE si.archive_id = ?1
             LIMIT 1",
            params![archive_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|e| format!("查询画集所属合集失败: {e}"))
}
