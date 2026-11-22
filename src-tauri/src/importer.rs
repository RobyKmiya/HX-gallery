use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::Instant;

use rusqlite::{Connection, OptionalExtension};
use tauri::{AppHandle, Emitter};
use uuid::Uuid;
use walkdir::WalkDir;

use crate::archive;
use crate::db;
use crate::fsutil;
use crate::models::{
    BulkTagReport, ImportErrorItem, ImportOptions, ImportProgress, ImportReport, TagDraft,
};

struct Candidate {
    source: PathBuf,
    base: PathBuf,
}

/// 导入面板的一行：把用户给的一批路径展开成实际的压缩包清单。
pub struct StagedInput {
    pub source: PathBuf,
    pub in_library: bool,
    pub issue: Option<String>,
}

/// 只做展开和轻量校验，不复制、不读压缩包内容（页数由调用方按需再数）。
pub fn gather_inputs(root: &Path, inputs: &[PathBuf]) -> (Vec<StagedInput>, Vec<ImportErrorItem>) {
    let mut errors = Vec::new();
    let candidates = collect_candidates(inputs, &mut errors);

    let staged = candidates
        .into_iter()
        .map(|candidate| {
            let relative = relative_of(root, &candidate.source);
            let inside_gallery = relative
                .as_deref()
                .map(|rel| rel == ".gallery" || rel.starts_with(".gallery/"))
                .unwrap_or(false);
            StagedInput {
                in_library: relative.is_some(),
                issue: inside_gallery
                    .then(|| "位于画廊自身的 .gallery 目录内，不能导入".to_string()),
                source: candidate.source,
            }
        })
        .collect();

    (staged, errors)
}

/// 写入一条画集记录所需的字段。参数较多，集中成一个结构体避免调用处错位。
struct ArchiveInsert<'a> {
    rel_path: &'a str,
    file_name: &'a str,
    extension: &'a str,
    file_size: i64,
    file_mtime: i64,
    content_hash: &'a str,
    page_count: i64,
    cover_key: Option<&'a str>,
    inspection_error: Option<&'a str>,
}

pub fn import_blocking(
    app: &AppHandle,
    root: &Path,
    covers_dir: &Path,
    inputs: Vec<PathBuf>,
    options: ImportOptions,
) -> ImportReport {
    import_with_progress(root, covers_dir, inputs, options, |progress| {
        let _ = app.emit("import-progress", progress);
    })
}

/// 导入入口：自行打开一条「只做连接配置、不跑迁移」的工作连接。
pub fn import_with_progress<F>(
    root: &Path,
    covers_dir: &Path,
    inputs: Vec<PathBuf>,
    options: ImportOptions,
    on_progress: F,
) -> ImportReport
where
    F: FnMut(ImportProgress),
{
    let connection = match db::open_worker_connection(root) {
        Ok(connection) => connection,
        Err(error) => return import_failure(root, error),
    };
    import_on_connection(&connection, root, covers_dir, inputs, options, on_progress)
}

/// 复用调用方已打开的连接导入（桌面命令与 CLI 走这条路径）。
pub fn import_on_connection<F>(
    connection: &Connection,
    root: &Path,
    covers_dir: &Path,
    inputs: Vec<PathBuf>,
    options: ImportOptions,
    mut on_progress: F,
) -> ImportReport
where
    F: FnMut(ImportProgress),
{
    let started = Instant::now();
    let mut errors = Vec::new();
    let candidates = collect_candidates(&inputs, &mut errors);

    // 收集阶段（路径不存在、格式不支持等）已经失败的条目也要计入总数，
    // 否则进度里会出现「失败数 > 总数」的矛盾读数。
    let collected_failures = errors.len();
    let total = candidates.len() + collected_failures;

    let temp_root = root
        .join(".gallery")
        .join("tmp")
        .join(Uuid::new_v4().to_string());
    if let Err(error) = fs::create_dir_all(&temp_root) {
        return ImportReport {
            imported: 0,
            skipped: 0,
            failed: 1,
            errors: vec![ImportErrorItem {
                file: root.to_string_lossy().to_string(),
                message: error.to_string(),
            }],
            message: format!("无法创建导入临时目录：{error}"),
            archive_ids: Vec::new(),
        };
    }

    let mut imported = 0usize;
    let mut skipped = 0usize;
    let mut failed = collected_failures;
    let mut archive_ids: Vec<i64> = Vec::new();
    let preserve_structure = options.preserve_structure.unwrap_or(false);

    emit_progress(
        &mut on_progress,
        "start",
        0,
        total,
        "",
        imported,
        skipped,
        failed,
    );

    for (index, candidate) in candidates.iter().enumerate() {
        let current = index + 1 + collected_failures;
        let display_name = candidate
            .source
            .file_name()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_else(|| candidate.source.to_string_lossy().to_string());

        emit_progress(
            &mut on_progress,
            "processing",
            current,
            total,
            &display_name,
            imported,
            skipped,
            failed,
        );

        let metadata = match fs::metadata(&candidate.source) {
            Ok(metadata) => metadata,
            Err(error) => {
                failed += 1;
                errors.push(ImportErrorItem {
                    file: candidate.source.to_string_lossy().to_string(),
                    message: format!("无法读取文件信息: {error}"),
                });
                continue;
            }
        };

        let file_size = metadata.len() as i64;
        let file_mtime = fsutil::modified_ms(&metadata);

        let relative_source = relative_of(root, &candidate.source);
        let source_in_library = relative_source.is_some();

        if let Some(relative) = relative_source.as_deref() {
            if relative == ".gallery" || relative.starts_with(".gallery/") {
                failed += 1;
                errors.push(ImportErrorItem {
                    file: candidate.source.to_string_lossy().to_string(),
                    message: "不能导入画廊自身的 .gallery 目录".to_string(),
                });
                continue;
            }
        }

        let mut temporary_path: Option<PathBuf> = None;
        let (content_hash, copied_size) = if source_in_library {
            match fsutil::hash_file(&candidate.source) {
                Ok(hash) => (hash, file_size as u64),
                Err(error) => {
                    failed += 1;
                    errors.push(ImportErrorItem {
                        file: candidate.source.to_string_lossy().to_string(),
                        message: error,
                    });
                    continue;
                }
            }
        } else {
            let temp_path = temp_root.join(format!("{index}-{}.part", Uuid::new_v4()));
            match fsutil::copy_and_hash(&candidate.source, &temp_path) {
                Ok((hash, size)) => {
                    temporary_path = Some(temp_path);
                    (hash, size)
                }
                Err(error) => {
                    let _ = fs::remove_file(&temp_path);
                    failed += 1;
                    errors.push(ImportErrorItem {
                        file: candidate.source.to_string_lossy().to_string(),
                        message: error,
                    });
                    continue;
                }
            }
        };

        if copied_size != file_size as u64 {
            if let Some(temp) = temporary_path.take() {
                let _ = fs::remove_file(temp);
            }
            failed += 1;
            errors.push(ImportErrorItem {
                file: candidate.source.to_string_lossy().to_string(),
                message: "复制后的文件大小与原文件不一致".to_string(),
            });
            continue;
        }

        match db::find_duplicate(connection, &content_hash) {
            Ok(Some(existing_id)) => {
                if let Some(temp) = temporary_path.take() {
                    let _ = fs::remove_file(temp);
                }
                if let Some(collection_id) = options.add_to_collection {
                    // 原来这里用 `let _ =` 吞掉了错误：用户以为已加入收藏夹，实际没有。
                    if let Err(error) =
                        db::add_to_collection(connection, collection_id, existing_id)
                    {
                        failed += 1;
                        errors.push(ImportErrorItem {
                            file: display_name.clone(),
                            message: format!("重复文件未能加入收藏夹: {error}"),
                        });
                        continue;
                    }
                }
                // 重复的那本虽然不复制，但用户在导入面板里填的标签是有意义的：
                // 直接丢掉会让人以为「填了标签却没生效」。这里补到已有记录上。
                let path_tags = options.tags_for(&candidate.source);
                if !path_tags.is_empty() {
                    match db::add_tags_bulk(connection, existing_id, path_tags) {
                        Ok(report) => {
                            for failure in report.failed {
                                errors.push(ImportErrorItem {
                                    file: display_name.clone(),
                                    message: format!(
                                        "标签「{}」未能添加：{}",
                                        failure.name, failure.reason
                                    ),
                                });
                            }
                        }
                        Err(error) => errors.push(ImportErrorItem {
                            file: display_name.clone(),
                            message: format!("重复文件未能补加标签: {error}"),
                        }),
                    }
                }
                skipped += 1;
                emit_progress(
                    &mut on_progress,
                    "skipped",
                    current,
                    total,
                    &display_name,
                    imported,
                    skipped,
                    failed,
                );
                continue;
            }
            Ok(None) => {}
            Err(error) => {
                if let Some(temp) = temporary_path.take() {
                    let _ = fs::remove_file(temp);
                }
                failed += 1;
                errors.push(ImportErrorItem {
                    file: candidate.source.to_string_lossy().to_string(),
                    message: error,
                });
                continue;
            }
        }

        let target = if let Some(relative) = &relative_source {
            match fsutil::join_rel(root, relative) {
                Some(path) => path,
                None => {
                    failed += 1;
                    errors.push(ImportErrorItem {
                        file: candidate.source.to_string_lossy().to_string(),
                        message: "无法解析库内文件路径".to_string(),
                    });
                    continue;
                }
            }
        } else {
            let relative_target = target_relative(candidate, preserve_structure);
            let target = match unique_target(connection, root, &relative_target) {
                Ok(target) => target,
                Err(error) => {
                    if let Some(temp) = temporary_path.take() {
                        let _ = fs::remove_file(temp);
                    }
                    failed += 1;
                    errors.push(ImportErrorItem {
                        file: candidate.source.to_string_lossy().to_string(),
                        message: error,
                    });
                    continue;
                }
            };

            if let Some(parent) = target.parent() {
                if let Err(error) = fs::create_dir_all(parent) {
                    if let Some(temp) = temporary_path.take() {
                        let _ = fs::remove_file(temp);
                    }
                    failed += 1;
                    errors.push(ImportErrorItem {
                        file: candidate.source.to_string_lossy().to_string(),
                        message: format!("无法创建目标目录: {error}"),
                    });
                    continue;
                }
            }

            // 外部文件必然已经复制出临时文件；这里用显式分支替代原来的 expect()，
            // 避免未来重构破坏隐含不变式时直接 panic（release 下 panic = abort）。
            let Some(temp) = temporary_path.take() else {
                failed += 1;
                errors.push(ImportErrorItem {
                    file: candidate.source.to_string_lossy().to_string(),
                    message: "内部状态异常：缺少导入临时文件".to_string(),
                });
                continue;
            };
            if let Err(error) = fs::rename(&temp, &target) {
                let _ = fs::remove_file(&temp);
                failed += 1;
                errors.push(ImportErrorItem {
                    file: candidate.source.to_string_lossy().to_string(),
                    message: format!("移动到库目录失败: {error}"),
                });
                continue;
            }

            target
        };

        let relative_path = match fsutil::rel_to_string(root, &target) {
            Some(relative) => relative,
            None => {
                if temporary_path.is_none() && !source_in_library {
                    let _ = fs::remove_file(&target);
                }
                failed += 1;
                errors.push(ImportErrorItem {
                    file: candidate.source.to_string_lossy().to_string(),
                    message: "无法生成库内相对路径".to_string(),
                });
                continue;
            }
        };

        let inspection = archive::inspect_archive(&target, covers_dir, &content_hash);
        let file_name = target
            .file_name()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_else(|| display_name.clone());
        let extension = target
            .extension()
            .map(|value| value.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();

        let insert = ArchiveInsert {
            rel_path: &relative_path,
            file_name: &file_name,
            extension: &extension,
            file_size,
            file_mtime,
            content_hash: &content_hash,
            page_count: inspection.page_count,
            cover_key: inspection.cover_key.as_deref(),
            inspection_error: inspection.error.as_deref(),
        };

        // 写库、加入收藏夹、挂标签在同一个事务里：任一步失败都回滚，
        // 不会留下「导入成功但没标签」或「进了收藏夹但没记录」的半成品。
        match insert_archive_atomic(
            connection,
            &insert,
            options.add_to_collection,
            options.tags_for(&candidate.source),
        ) {
            Ok((new_id, tag_report)) => {
                archive_ids.push(new_id);
                // 单条标签失败不影响这本画集的导入，但必须如实报出来。
                for failure in tag_report.failed {
                    errors.push(ImportErrorItem {
                        file: file_name.clone(),
                        message: format!("标签「{}」未能添加：{}", failure.name, failure.reason),
                    });
                }
                imported += 1
            }
            Err(error) => {
                if !source_in_library {
                    let _ = fs::remove_file(&target);
                }
                failed += 1;
                errors.push(ImportErrorItem {
                    file: file_name,
                    message: error,
                });
            }
        }

        emit_progress(
            &mut on_progress,
            "processed",
            current,
            total,
            &display_name,
            imported,
            skipped,
            failed,
        );
    }

    let _ = fs::remove_dir_all(&temp_root);
    emit_progress(
        &mut on_progress,
        "done",
        total,
        total,
        "",
        imported,
        skipped,
        failed,
    );

    let elapsed = started.elapsed().as_millis();
    let message = format!(
        "导入完成：成功 {imported}，跳过重复 {skipped}，失败 {failed}，用时 {:.1}s",
        elapsed as f64 / 1000.0
    );

    ImportReport {
        imported,
        skipped,
        failed,
        errors,
        message,
        archive_ids,
    }
}

fn import_failure(root: &Path, error: String) -> ImportReport {
    ImportReport {
        imported: 0,
        skipped: 0,
        failed: 1,
        errors: vec![ImportErrorItem {
            file: root.to_string_lossy().to_string(),
            message: error.clone(),
        }],
        message: format!("无法打开库：{error}"),
        archive_ids: Vec::new(),
    }
}

fn insert_archive_atomic(
    connection: &Connection,
    insert: &ArchiveInsert<'_>,
    collection_id: Option<i64>,
    tags: &[TagDraft],
) -> Result<(i64, BulkTagReport), String> {
    let transaction = db::begin_write(connection)?;
    let archive_id = db::insert_archive(
        &transaction,
        insert.rel_path,
        insert.file_name,
        insert.extension,
        insert.file_size,
        insert.file_mtime,
        insert.content_hash,
        insert.page_count,
        insert.cover_key,
        insert.inspection_error,
        fsutil::now_ms(),
    )?;

    if let Some(collection_id) = collection_id {
        db::add_to_collection(&transaction, collection_id, archive_id)
            .map_err(|error| format!("加入收藏夹失败: {error}"))?;
    }

    let tag_report = if tags.is_empty() {
        BulkTagReport::default()
    } else {
        db::link_tags_in_tx(&transaction, archive_id, tags)
            .map_err(|error| format!("挂标签失败: {error}"))?
    };

    transaction
        .commit()
        .map_err(|error| format!("提交导入记录失败: {error}"))?;
    Ok((archive_id, tag_report))
}

fn collect_candidates(inputs: &[PathBuf], errors: &mut Vec<ImportErrorItem>) -> Vec<Candidate> {
    let mut candidates = Vec::new();
    let mut seen = HashSet::new();

    for input in inputs {
        let absolute = std::path::absolute(input).unwrap_or_else(|_| input.clone());
        let metadata = match fs::metadata(&absolute) {
            Ok(metadata) => metadata,
            Err(error) => {
                errors.push(ImportErrorItem {
                    file: input.to_string_lossy().to_string(),
                    message: format!("无法读取路径: {error}"),
                });
                continue;
            }
        };

        if metadata.is_file() {
            if archive::is_supported_archive(&absolute) {
                let base = absolute
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| absolute.clone());
                push_candidate(&mut candidates, &mut seen, absolute, base);
            } else {
                errors.push(ImportErrorItem {
                    file: input.to_string_lossy().to_string(),
                    message: "不支持的文件格式，目前仅支持 ZIP/CBZ/7Z/RAR".to_string(),
                });
            }
            continue;
        }

        if !metadata.is_dir() {
            errors.push(ImportErrorItem {
                file: input.to_string_lossy().to_string(),
                message: "不是普通文件或目录".to_string(),
            });
            continue;
        }

        for entry in WalkDir::new(&absolute)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| entry.file_name() != ".gallery")
        {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    errors.push(ImportErrorItem {
                        file: input.to_string_lossy().to_string(),
                        message: format!("遍历目录失败: {error}"),
                    });
                    continue;
                }
            };

            if entry.file_type().is_file() && archive::is_supported_archive(entry.path()) {
                push_candidate(
                    &mut candidates,
                    &mut seen,
                    entry.path().to_path_buf(),
                    absolute.clone(),
                );
            }
        }
    }

    candidates.sort_by(|left, right| {
        left.source
            .to_string_lossy()
            .to_ascii_lowercase()
            .cmp(&right.source.to_string_lossy().to_ascii_lowercase())
    });
    candidates
}

fn push_candidate(
    candidates: &mut Vec<Candidate>,
    seen: &mut HashSet<PathBuf>,
    source: PathBuf,
    base: PathBuf,
) {
    let key = fs::canonicalize(&source).unwrap_or_else(|_| source.clone());
    if seen.insert(key) {
        candidates.push(Candidate { source, base });
    }
}

fn target_relative(candidate: &Candidate, preserve_structure: bool) -> PathBuf {
    if preserve_structure {
        if let Ok(relative) = candidate.source.strip_prefix(&candidate.base) {
            let parts: Vec<String> = relative
                .components()
                .filter_map(|component| match component {
                    Component::Normal(value) => Some(value.to_string_lossy().to_string()),
                    _ => None,
                })
                .map(|value| fsutil::sanitize_name(&value, false))
                .collect();

            if !parts.is_empty() {
                let mut path = PathBuf::new();
                for part in parts {
                    path.push(part);
                }
                return path;
            }
        }
    }

    let file_name = candidate
        .source
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| "未命名.zip".to_string());

    PathBuf::from(fsutil::sanitize_name(&file_name, false))
}

fn unique_target(connection: &Connection, root: &Path, relative: &Path) -> Result<PathBuf, String> {
    for suffix in 1..10_000 {
        let candidate_relative = if suffix == 1 {
            relative.to_path_buf()
        } else {
            fsutil::path_with_suffix(relative, suffix)
        };
        let candidate = root.join(&candidate_relative);
        let relative_string = fsutil::rel_to_string(root, &candidate)
            .ok_or_else(|| "无法生成目标相对路径".to_string())?;

        let exists_in_db: Option<i64> = connection
            .query_row(
                "SELECT id FROM archives WHERE rel_path = ?1",
                rusqlite::params![relative_string],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| format!("检查目标路径失败: {error}"))?;

        if !candidate.exists() && exists_in_db.is_none() {
            return Ok(candidate);
        }
    }

    Err("无法生成不冲突的目标文件名".to_string())
}

fn relative_of(root: &Path, path: &Path) -> Option<String> {
    fsutil::rel_to_string(root, path).or_else(|| {
        let canonical_root = fs::canonicalize(root).ok()?;
        let canonical_path = fs::canonicalize(path).ok()?;
        fsutil::rel_to_string(&canonical_root, &canonical_path)
    })
}

#[allow(clippy::too_many_arguments)]
fn emit_progress<F>(
    on_progress: &mut F,
    phase: &str,
    current: usize,
    total: usize,
    file_name: &str,
    imported: usize,
    skipped: usize,
    failed: usize,
) where
    F: FnMut(ImportProgress),
{
    on_progress(ImportProgress {
        current,
        total,
        file_name: file_name.to_string(),
        imported,
        skipped,
        failed,
        phase: phase.to_string(),
    });
}
