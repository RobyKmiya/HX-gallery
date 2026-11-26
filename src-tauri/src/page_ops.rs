//! 画廊单页增删（管理员 2026-11-26：能单独增删画廊的图片）。
//!
//! 画匣的画集都是 zip/cbz 容器，页不是独立文件，所以「删一页 / 加几页」
//! 的实现方式是**重打包**：
//! - 删除：按阅读器同样的顺序找到第 N 页对应的原始条目，其余条目原样复制进
//!   新包，原子替换旧文件；被删的那一页先落到 `.gallery/trash/pages/{id}/`
//!   （回收站里二次删除才根除）。
//! - 添加：原生文件选择器挑图片，追加到包尾。
//! - 只支持 zip/cbz；7z/RAR 加密包直接给出明确提示。
//!
//! 防卡顿要点：整个重打包在**后台线程**做，进度经 `gallery://pageops` 事件
//! 发给前端；页/清单缓存键自带「大小+修改时间」，包被替换后旧键自然失效，
//! 不需要手动清理（见 pagecache.rs 模块注释）。

use rusqlite::params;
use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};
use tauri::Emitter;

/// 重打包进度事件。
fn emit_progress(app: &tauri::AppHandle, archive_id: i64, stage: &str, done: usize, total: usize) {
    let _ = app.emit(
        "gallery://pageops",
        serde_json::json!({
            "archiveId": archive_id,
            "stage": stage,
            "done": done,
            "total": total,
        }),
    );
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PageEntry {
    pub index: i64,
    pub name: String,
    pub size: i64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PageList {
    /// "zip" = 可增删；"unsupported" = 7z/RAR 等。
    pub kind: String,
    pub encrypted: bool,
    pub pages: Vec<PageEntry>,
    pub page_count: i64,
}

struct ArchiveRow {
    abs: PathBuf,
    ext: String,
    password: Option<String>,
    page_count: i64,
    last_page: i64,
}

fn load_archive(
    conn: &rusqlite::Connection,
    root: &Path,
    id: i64,
) -> Result<ArchiveRow, String> {
    let (rel_path, ext, password, page_count, last_page): (
        String,
        String,
        Option<String>,
        i64,
        i64,
    ) = conn
        .query_row(
            "SELECT rel_path, ext, password, page_count, last_page
             FROM archives WHERE id = ?1 AND deleted_at IS NULL",
            params![id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .map_err(|e| format!("画集不存在或已删除：{e}"))?;
    Ok(ArchiveRow {
        abs: root.join(&rel_path),
        ext,
        password,
        page_count,
        last_page,
    })
}

fn repackable(ext: &str) -> bool {
    matches!(ext, "zip" | "cbz")
}

/// 阅读器同款顺序的图片条目（过滤 + 自然排序）。
fn image_entries(path: &Path) -> Result<Vec<crate::container::ContainerEntry>, String> {
    let mut entries: Vec<crate::container::ContainerEntry> =
        crate::container::list_entries(path)?
        .into_iter()
        .filter(|entry| {
            !crate::archive::is_ignored_entry(&entry.name)
                && crate::archive::is_image_name(&entry.name)
        })
        .collect();
    entries.sort_by(|left, right| natord::compare(&left.name, &right.name));
    Ok(entries)
}

/// 读一条原始条目的字节（带密码）。
fn read_raw(
    path: &Path,
    raw_index: usize,
    password: Option<&str>,
) -> Result<Vec<u8>, String> {
    crate::container::read_entry_with_password(path, raw_index, password)
}

/// 重打包核心：`skip_raw` 不为空时跳过该原始条目，`append` 追加新文件。
/// 完成后原子替换旧包，返回（新页数, 新文件大小）。
fn repack(
    app: &tauri::AppHandle,
    archive_id: i64,
    path: &Path,
    password: Option<&str>,
    skip_raw: Option<usize>,
    append: Vec<(String, Vec<u8>)>,
    removed_to: Option<&Path>,
) -> Result<(i64, u64), String> {
    let raw = crate::container::list_entries(path)?;
    let total = raw.len() + append.len();
    let mut done = 0usize;

    let tmp = path.with_file_name(format!(
        "{}.repack-{pid}.zip",
        path.file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("archive"),
        pid = std::process::id()
    ));

    let write_result = (|| -> Result<(), String> {
        let file = std::fs::File::create(&tmp).map_err(|e| format!("建临时包失败：{e}"))?;
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .large_file(true);

        for item in &raw {
            if skip_raw == Some(item.raw_index) {
                // 被删的页先落回收站：二次删除才根除。
                if let Some(target_dir) = removed_to {
                    if let Ok(bytes) = read_raw(path, item.raw_index, password) {
                        let _ = std::fs::create_dir_all(target_dir);
                        let safe = item.name.replace(['/', '\\'], "_");
                        let _ = std::fs::write(target_dir.join(safe), bytes);
                    }
                }
                done += 1;
                continue;
            }
            let bytes = read_raw(path, item.raw_index, password)?;
            writer
                .start_file(item.name.as_str(), options)
                .map_err(|e| format!("写入条目失败：{e}"))?;
            writer
                .write_all(&bytes)
                .map_err(|e| format!("写入条目内容失败：{e}"))?;
            done += 1;
            if done % 12 == 0 {
                emit_progress(app, archive_id, "重打包中", done, total);
            }
        }

        for (name, bytes) in &append {
            writer
                .start_file(name.as_str(), options)
                .map_err(|e| format!("写入新页失败：{e}"))?;
            writer
                .write_all(bytes)
                .map_err(|e| format!("写入新页内容失败：{e}"))?;
            done += 1;
            emit_progress(app, archive_id, "写入新页", done, total);
        }

        writer
            .finish()
            .map_err(|e| format!("收尾压缩包失败：{e}"))?;
        Ok(())
    })();

    if let Err(error) = write_result {
        let _ = std::fs::remove_file(&tmp);
        return Err(error);
    }

    // Windows 上 rename 不覆盖：先移走旧包。
    let backup = path.with_file_name(format!(
        "{}.old-{pid}",
        path.file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("archive"),
        pid = std::process::id()
    ));
    let _ = std::fs::remove_file(&backup);
    std::fs::rename(path, &backup).map_err(|e| format!("旧包换名失败：{e}"))?;
    if let Err(error) = std::fs::rename(&tmp, path) {
        // 换不回去要把旧包恢复，不然画集直接没了。
        let _ = std::fs::rename(&backup, path);
        return Err(format!("新包落位失败：{error}"));
    }
    let _ = std::fs::remove_file(&backup);

    let new_size = std::fs::metadata(path)
        .map(|meta| meta.len())
        .unwrap_or(0);
    emit_progress(app, archive_id, "完成", total, total);
    let new_count = total as i64 - append.len() as i64 - (skip_raw.is_some() as i64)
        + append.len() as i64;
    Ok((new_count, new_size))
}

/// 页清单（前端「图片管理」用）。
pub(crate) fn page_list(
    conn: &rusqlite::Connection,
    root: &Path,
    id: i64,
) -> Result<PageList, String> {
    let row = load_archive(conn, root, id)?;
    if !repackable(&row.ext) {
        return Ok(PageList {
            kind: "unsupported".to_string(),
            encrypted: row.password.is_some(),
            pages: Vec::new(),
            page_count: row.page_count,
        });
    }
    let images = image_entries(&row.abs)?;
    let pages = images
        .iter()
        .enumerate()
        .map(|(index, entry)| PageEntry {
            index: index as i64,
            name: entry.name.clone(),
            size: entry.size as i64,
        })
        .collect();
    Ok(PageList {
        kind: "zip".to_string(),
        encrypted: row.password.is_some(),
        pages,
        page_count: row.page_count,
    })
}

/// 删除第 `index` 页（0 基，阅读器同序）。返回剩余页数。
#[allow(clippy::too_many_arguments)]
pub(crate) fn page_delete(
    app: &tauri::AppHandle,
    conn: &rusqlite::Connection,
    root: &Path,
    id: i64,
    index: i64,
) -> Result<i64, String> {
    let row = load_archive(conn, root, id)?;
    if !repackable(&row.ext) {
        return Err(format!(
            "{} 包暂不支持单页增删（只支持 zip/cbz）；可以先解压再重新打包",
            row.ext.to_uppercase()
        ));
    }
    let images = image_entries(&row.abs)?;
    if images.len() <= 1 {
        return Err("只剩最后一页了，不能删".to_string());
    }
    let Some(entry) = images.get(index.max(0) as usize) else {
        return Err("页码超出了范围".to_string());
    };
    let trash_dir = root.join(".gallery").join("trash").join("pages").join(id.to_string());
    let (new_count, new_size) = repack(
        app,
        id,
        &row.abs,
        row.password.as_deref(),
        Some(entry.raw_index),
        Vec::new(),
        Some(&trash_dir),
    )?;

    let new_last = row.last_page.min(new_count.saturating_sub(1));
    conn.execute(
        "UPDATE archives SET page_count = ?1, last_page = ?2, file_size = ?3,
         file_mtime = ?4 WHERE id = ?5",
        params![
            new_count,
            new_last,
            new_size as i64,
            crate::fsutil::modified_ms(&std::fs::metadata(&row.abs).map_err(|e| format!("读文件信息失败：{e}"))?),
            id
        ],
    )
    .map_err(|e| format!("更新页数失败：{e}"))?;
    Ok(new_count)
}

/// 追加图片（原生选择器挑文件）。返回（新增页数, 总页数）。
pub(crate) fn page_add(
    app: &tauri::AppHandle,
    conn: &rusqlite::Connection,
    root: &Path,
    id: i64,
) -> Result<(usize, i64), String> {
    let row = load_archive(conn, root, id)?;
    if !repackable(&row.ext) {
        return Err(format!(
            "{} 包暂不支持单页增删（只支持 zip/cbz）；可以先解压再重新打包",
            row.ext.to_uppercase()
        ));
    }
    let existing = image_entries(&row.abs)?;
    let mut taken: std::collections::HashSet<String> =
        existing.iter().map(|entry| entry.name.clone()).collect();

    use tauri_plugin_dialog::DialogExt;
    let app_for_dialog = app.clone();
    let picked = std::thread::spawn(move || {
        app_for_dialog.dialog()
            .file()
            .add_filter("图片", &["jpg", "jpeg", "png", "webp", "gif", "bmp", "avif"])
            .blocking_pick_files()
    })
    .join()
    .map_err(|_| "选择器线程异常".to_string())?;

    let Some(files) = picked else {
        return Err("没有选择图片".to_string());
    };
    let mut append: Vec<(String, Vec<u8>)> = Vec::new();
    for file in files {
        let Ok(path) = file.into_path() else { continue };
        let Some(raw_name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if !crate::archive::is_image_name(raw_name) {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let mut name = raw_name.to_string();
        let mut suffix = 1usize;
        while taken.contains(&name) {
            let stem = Path::new(raw_name)
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or("page");
            let ext = Path::new(raw_name)
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or("png");
            name = format!("{stem}_add{suffix:02}.{ext}");
            suffix += 1;
        }
        taken.insert(name.clone());
        append.push((name, bytes));
    }
    if append.is_empty() {
        return Err("没有可导入的图片（只认 jpg/png/webp/gif/bmp/avif）".to_string());
    }
    let added = append.len();

    let (new_count, new_size) = repack(
        app, id, &row.abs, row.password.as_deref(), None, append, None,
    )?;
    let new_last = row.last_page.min(new_count.saturating_sub(1));
    conn.execute(
        "UPDATE archives SET page_count = ?1, last_page = ?2, file_size = ?3,
         file_mtime = ?4 WHERE id = ?5",
        params![
            new_count,
            new_last,
            new_size as i64,
            crate::fsutil::modified_ms(&std::fs::metadata(&row.abs).map_err(|e| format!("读文件信息失败：{e}"))?),
            id
        ],
    )
    .map_err(|e| format!("更新页数失败：{e}"))?;
    Ok((added, new_count))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repackable_only_zip_family() {
        assert!(repackable("zip"));
        assert!(repackable("cbz"));
        assert!(!repackable("7z"));
        assert!(!repackable("rar"));
        assert!(!repackable(""));
    }
}
