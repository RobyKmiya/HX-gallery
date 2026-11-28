//! 备份与迁移（1.2.5，管理员提案 M2）。
//!
//! 三层能力：
//! 1. **手动导出 zip**：库快照（VACUUM INTO 一致性副本）+ settings.json 全量 +
//!    manifest，落 `AppConfig/backups/`——媒体本体不打包（几十 GB），只备元数据与配置；
//! 2. **整体恢复**：先做 pre-restore 安全快照（当前库+设置），再替换库与设置，
//!    恢复完自动重开库；裸 .db 日备也支持只恢复库；
//! 3. **每日自动设置快照**：库 db 日备已有（db.rs daily_backup，保留份数提到 7），
//!    这里补 settings.json 的每日快照（1.2.3 壁纸锁配置丢失事故的根治）。

use serde::Serialize;
use std::fs;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

use crate::db;
use crate::AppState;

const KEEP_DAILY: usize = 7;

fn backups_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|error| format!("无法获取配置目录: {error}"))?
        .join("backups");
    fs::create_dir_all(&dir).map_err(|error| format!("无法创建备份目录: {error}"))?;
    Ok(dir)
}

fn gallery_dir(root: &Path) -> PathBuf {
    root.join(".gallery")
}

fn prune(dir: &Path, prefix: &str, suffix: &str, keep: usize) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(prefix) && name.ends_with(suffix))
        })
        .collect();
    // 文件名内嵌时间戳，字典序即时间序。
    files.sort();
    while files.len() > keep {
        let oldest = files.remove(0);
        let _ = fs::remove_file(&oldest);
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct BackupReport {
    pub path: String,
    pub archive_count: i64,
    pub library_path: String,
}

/// 手动一键备份：库快照 + settings.json + manifest → zip。
pub fn export_backup(app: &AppHandle) -> Result<BackupReport, String> {
    let state = app.state::<AppState>();
    let inner = state.lock().map_err(|error| format!("状态表锁定失败: {error}"))?;
    let library = inner.library.as_ref().ok_or("库未打开，无法备份")?;
    let root = library.root.clone();
    let archive_count = library
        .conn
        .query_row("SELECT COUNT(*) FROM archives", [], |row| row.get::<_, i64>(0))
        .unwrap_or(-1);
    let library_id = library.library_id.clone();
    let library_name = library.library_name.clone();

    let dir = backups_dir(app)?;
    let stamp = crate::fsutil::now_ms();

    // VACUUM INTO：干净一致性快照（不直接拷热 WAL 文件）。
    let snap = dir.join(format!(".tmp-library-{stamp}.db"));
    let target = snap.to_string_lossy().to_string();
    library
        .conn
        .execute("VACUUM INTO ?1", [target])
        .map_err(|error| format!("库快照失败: {error}"))?;

    let settings_raw = fs::read_to_string(crate::settings_path(app)?)
        .map_err(|error| format!("读取 settings.json 失败: {error}"))?;

    let zip_path = dir.join(format!("backup-{stamp}.zip"));
    let file = fs::File::create(&zip_path)
        .map_err(|error| format!("无法创建备份文件: {error}"))?;
    let mut zip = zip::ZipWriter::new(file);
    let options: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();

    let version = app.package_info().version.to_string();
    let manifest = format!(
        r#"{{"created":{},"app_version":"{}","library_path":"{}","library_id":"{}","library_name":"{}","archive_count":{}}}"#,
        stamp,
        version.replace('\\', "\\\\").replace('"', "\\\""),
        root.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\""),
        library_id.replace('"', "\\\""),
        library_name.replace('"', "\\\""),
        archive_count
    );
    zip.start_file("manifest.json", options)
        .map_err(|error| format!("写入 manifest 失败: {error}"))?;
    zip.write_all(manifest.as_bytes())
        .map_err(|error| format!("写入 manifest 失败: {error}"))?;

    let db_bytes =
        fs::read(&snap).map_err(|error| format!("读取库快照失败: {error}"))?;
    zip.start_file("library.db", options)
        .map_err(|error| format!("写入库快照失败: {error}"))?;
    zip.write_all(&db_bytes)
        .map_err(|error| format!("写入库快照失败: {error}"))?;
    let _ = fs::remove_file(&snap);

    zip.start_file("settings.json", options)
        .map_err(|error| format!("写入设置失败: {error}"))?;
    zip.write_all(settings_raw.as_bytes())
        .map_err(|error| format!("写入设置失败: {error}"))?;

    zip.finish()
        .map_err(|error| format!("收尾备份失败: {error}"))?;

    // 超额清理：手动备份保留最近 10 份。
    prune(&dir, "backup-", ".zip", 10);

    Ok(BackupReport {
        path: zip_path.to_string_lossy().to_string(),
        archive_count,
        library_path: root.to_string_lossy().to_string(),
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct BackupEntry {
    pub name: String,
    pub path: String,
    pub size: u64,
    /// export（手动 zip）/ daily-db（每日库快照）/ daily-settings（每日设置快照）
    pub kind: String,
    pub created_ms: i64,
}

/// 列出全部可用备份：AppConfig/backups 的手动 zip + 库内 .gallery/backups 的每日档。
pub fn list_backups(app: &AppHandle) -> Vec<BackupEntry> {
    let mut items: Vec<BackupEntry> = Vec::new();
    let collect = |dir: &Path, prefix: &str, suffix: &str, kind: &str, items: &mut Vec<BackupEntry>| {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.filter_map(|entry| entry.ok()) {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !name.starts_with(prefix) || !name.ends_with(suffix) {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            let created_ms = meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_millis() as i64)
                .unwrap_or(0);
            items.push(BackupEntry {
                name: name.to_string(),
                path: path.to_string_lossy().to_string(),
                size: meta.len(),
                kind: kind.to_string(),
                created_ms,
            });
        }
    };
    if let Ok(dir) = backups_dir(app) {
        collect(&dir, "backup-", ".zip", "export", &mut items);
    }
    let state = app.state::<AppState>();
    if let Ok(inner) = state.lock() {
        if let Some(library) = &inner.library {
            let gallery = gallery_dir(&library.root);
            collect(&gallery.join("backups"), "library-daily-", ".db", "daily-db", &mut items);
            collect(
                &gallery.join("backups"),
                "settings-daily-",
                ".json",
                "daily-settings",
                &mut items,
            );
        }
    }
    items.sort_by(|a, b| b.created_ms.cmp(&a.created_ms));
    items
}

#[derive(Debug, Clone, Serialize)]
pub struct RestoreReport {
    pub restored_db: bool,
    pub restored_settings: bool,
    pub archive_count: i64,
    pub warnings: Vec<String>,
}

/// 整体恢复。先打 pre-restore 安全快照，再替换、重开库。
/// 支持：手动 zip（库+设置全恢复）、每日 .db（只恢复库）、每日 settings json（只恢复设置）。
pub fn restore_backup(app: &AppHandle, zip_path: &str) -> Result<RestoreReport, String> {
    let source = PathBuf::from(zip_path);
    if !source.exists() {
        return Err("备份文件不存在".to_string());
    }
    let name = source
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_string();
    let restore_db = name.starts_with("backup-") || name.starts_with("library-daily-");
    let restore_settings = name.starts_with("backup-") || name.starts_with("settings-daily-");
    if !restore_db && !restore_settings {
        return Err("无法识别的备份类型".to_string());
    }

    // ── 读备份内容 ──
    let mut db_bytes: Option<Vec<u8>> = None;
    let mut settings_bytes: Option<Vec<u8>> = None;
    let mut warnings: Vec<String> = Vec::new();
    if name.starts_with("backup-") {
        let file = fs::File::open(&source).map_err(|error| format!("打开备份失败: {error}"))?;
        let mut zip = zip::ZipArchive::new(file)
            .map_err(|error| format!("备份包读取失败: {error}"))?;
        for index in 0..zip.len() {
            let mut entry = zip
                .by_index(index)
                .map_err(|error| format!("备份包读取失败: {error}"))?;
            let entry_name = entry.name().to_string();
            let mut bytes = Vec::new();
            entry
                .read_to_end(&mut bytes)
                .map_err(|error| format!("备份包读取失败: {error}"))?;
            match entry_name.as_str() {
                "library.db" => db_bytes = Some(bytes),
                "settings.json" => settings_bytes = Some(bytes),
                _ => {}
            }
        }
        if db_bytes.is_none() {
            return Err("备份包里没有 library.db，无法恢复库".to_string());
        }
        if let Ok(manifest) = zip.by_name("manifest.json") {
            let text = std::io::read_to_string(manifest).unwrap_or_default();
            if !text.is_empty() && !text.contains(app.package_info().version.to_string().as_str()) {
                warnings.push(format!(
                    "备份来自另一版本（{}），恢复后建议重启",
                    text.split("\"app_version\":\"").nth(1)
                        .and_then(|s| s.split('"').next())
                        .unwrap_or("未知")
                ));
            }
        }
    } else {
        let bytes =
            fs::read(&source).map_err(|error| format!("读取备份失败: {error}"))?;
        if restore_db {
            db_bytes = Some(bytes);
        } else {
            settings_bytes = Some(bytes);
        }
    }

    // ── pre-restore 安全快照：当前库 + 当前设置先保命 ──
    let dir = backups_dir(app)?;
    let stamp = crate::fsutil::now_ms();
    let state = app.state::<AppState>();
    let mut inner = state.lock().map_err(|error| format!("状态表锁定失败: {error}"))?;
    let current_root: Option<PathBuf> = inner.library.as_ref().map(|library| library.root.clone());
    if restore_db {
        let Some(root) = current_root.clone() else {
            return Err("库未打开，无法恢复（先打开库）".to_string());
        };
        if let Some(library) = inner.library.take() {
            // 一致性快照当前库（conn 在 take 时已 drop，VACUUM 不行——直接对拷 db 文件）
            let snap_dir = dir.join(format!("pre-restore-{stamp}"));
            let _ = fs::create_dir_all(&snap_dir);
            let _ = fs::copy(
                gallery_dir(&root).join("library.db"),
                snap_dir.join("library.db"),
            );
            if let Ok(settings) = fs::read_to_string(crate::settings_path(app)?) {
                let _ = fs::write(snap_dir.join("settings.json"), settings);
            }
            warnings.push(format!(
                "恢复前快照：{}",
                snap_dir.to_string_lossy().to_string()
            ));
            let _ = library; // conn 已随 take 释放
        }
    }

    // ── 执行替换 ──
    let mut report = RestoreReport {
        restored_db: false,
        restored_settings: false,
        archive_count: 0,
        warnings: std::mem::take(&mut warnings),
    };

    if let Some(bytes) = settings_bytes {
        let path = crate::settings_path(app)?;
        fs::write(&path, &bytes)
            .map_err(|error| format!("写回 settings.json 失败: {error}"))?;
        report.restored_settings = true;
    }

    if let Some(bytes) = db_bytes {
        let Some(root) = current_root.clone() else {
            return Err("库未打开，无法恢复库（先打开库）".to_string());
        };
        let gallery = gallery_dir(&root);
        let db_path = gallery.join("library.db");
        // 旧连接已释放；残留 WAL/SHM 必须清掉，否则新 db 配旧 WAL 直接损坏。
        let _ = fs::remove_file(gallery.join("library.db-wal"));
        let _ = fs::remove_file(gallery.join("library.db-shm"));
        fs::write(&db_path, &bytes)
            .map_err(|error| format!("写回 library.db 失败: {error}"))?;
        report.restored_db = true;
    }

    // ── 重开库（conn 新建，读到的是恢复后的数据）──
    if report.restored_db {
        let Some(root) = current_root else {
            return Ok(report);
        };
        let library = db::open_library(&root)?;
        report.archive_count = library
            .conn
            .query_row("SELECT COUNT(*) FROM archives", [], |row| row.get::<_, i64>(0))
            .unwrap_or(-1);
        inner.library = Some(library);
        if report.restored_settings {
            warnings.push("设置已整体替换——重启画匣后浏览器等缓存设置完全生效".to_string());
        }
    }

    Ok(report)
}

/// Unix 秒 → (年, 月, 日)（民用历算法，proleptic Gregorian）。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// 每日 settings 快照（每天一份覆盖写，保留 7 份）。开库成功后调用。
pub fn daily_settings_snapshot(app: &AppHandle, root: &Path) {
    let Ok(settings_path) = crate::settings_path(app) else {
        return;
    };
    let Ok(settings) = fs::read_to_string(settings_path) else {
        return;
    };
    let dir = gallery_dir(root).join("backups");
    let _ = fs::create_dir_all(&dir);
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs() as i64)
        .unwrap_or(0);
    // UTC+8 换日再取日期，与国内直觉一致。
    let (year, month, day) = civil_from_days((secs + 8 * 3600) / 86_400);
    let today = format!("settings-daily-{year:04}{month:02}{day:02}");
    let _ = fs::write(dir.join(format!("{today}.json")), settings);
    prune(&dir, "settings-daily-", ".json", KEEP_DAILY);
    prune(&dir, "library-daily-", ".db", KEEP_DAILY);
}
