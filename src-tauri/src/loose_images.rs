//! 散图收藏夹：文件夹形式的多区单图收藏。
//!
//! 与「画集收藏夹」（collections，装画集）完全独立：这里收藏的是**单张图片**，
//! 有两种记录形态——
//! - `file`：指向磁盘上一个真实图片文件（管理员复制粘贴进绑定文件夹的、
//!   或从画廊「提取另存」落盘的）。画匣只记录路径，不迁移不复制；
//! - `archive_page`：指向画廊里某画集的某页（零空间），阅读协议现成。
//!
//! 服务走 `loose://img/<id>` 与 `loose://thumb/<id>` 自定义协议：按记录 id 从
//! 库里解析真实路径，杜绝路径穿越；缩略图懒生成进缓存（480px 上限）。
//! 评分与画集同刻度 0–100；收藏为图级红心标记，与区归属独立。

use rusqlite::Connection;
use serde::Serialize;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

use crate::db;
use crate::reader;

/// 散图文件扩展名白名单（扫描与收集共用）。
pub const IMAGE_EXTENSIONS: [&str; 7] = ["jpg", "jpeg", "png", "webp", "gif", "avif", "bmp"];
/// 缩略图最长边。
const THUMB_MAX_EDGE: u32 = 480;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LooseCollection {
    pub id: i64,
    pub name: String,
    pub folder: String,
    pub count: i64,
    pub favorites: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LooseImage {
    pub id: i64,
    pub collection_id: i64,
    /// "file"（磁盘文件引用）| "archive_page"（画廊源页引用）。
    pub kind: String,
    pub path: Option<String>,
    pub archive_id: Option<i64>,
    pub page_index: Option<i64>,
    /// archive_page 型的「源自」角标（画集标题）。
    pub archive_title: Option<String>,
    pub rating: i64,
    pub favorite: bool,
    pub file_size: Option<i64>,
    pub missing: bool,
    pub added_at: i64,
}

/// 校验区名：非空、≤64 字符、不含路径分隔符（区名会被用作默认文件夹名）。
fn validate_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("区名不能为空".to_string());
    }
    if trimmed.chars().count() > 64 {
        return Err("区名最多 64 个字符".to_string());
    }
    if trimmed.contains('/') || trimmed.contains('\\') || trimmed.contains(':') {
        return Err("区名不能包含路径分隔符".to_string());
    }
    Ok(trimmed.to_string())
}

fn is_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            IMAGE_EXTENSIONS
                .iter()
                .any(|allowed| ext.eq_ignore_ascii_case(allowed))
        })
        .unwrap_or(false)
}

fn mime_for_ext(ext: &str) -> &'static str {
    match ext.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        _ => "application/octet-stream",
    }
}

/// 新建一个区：默认在根目录下建同名文件夹；也允许绑定一个**已存在**的文件夹。
pub fn create_collection(
    conn: &mut Connection,
    name: &str,
    folder: Option<&Path>,
    loose_root: &Path,
) -> Result<LooseCollection, String> {
    let name = validate_name(name)?;
    let exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM image_collections WHERE name = ?1",
            [name.as_str()],
            |row| row.get(0),
        )
        .map_err(|e| format!("查询散图区失败: {e}"))?;
    if exists > 0 {
        return Err(format!("已有叫「{name}」的散图区"));
    }

    let folder = match folder {
        Some(path) => {
            // 绑定已有文件夹：必须真实存在（防手滑路径让扫描永远空转）
            if !path.is_dir() {
                return Err("文件夹不存在，无法绑定".to_string());
            }
            path.to_path_buf()
        }
        None => {
            let path = loose_root.join(&name);
            fs::create_dir_all(&path).map_err(|e| format!("创建文件夹失败: {e}"))?;
            path
        }
    };

    let now = crate::fsutil::now_ms();
    conn.execute(
        "INSERT INTO image_collections(name, folder, created_at) VALUES (?1, ?2, ?3)",
        rusqlite::params![name, folder.to_string_lossy(), now],
    )
    .map_err(|e| format!("创建散图区失败: {e}"))?;
    let id = conn.last_insert_rowid();
    collection_by_id(conn, id)
}

fn collection_by_id(conn: &Connection, id: i64) -> Result<LooseCollection, String> {
    list_collections(conn)?
        .into_iter()
        .find(|collection| collection.id == id)
        .ok_or_else(|| "散图区创建后读取失败".to_string())
}

/// 列出全部散图区（带张数与收藏数）。
pub fn list_collections(conn: &Connection) -> Result<Vec<LooseCollection>, String> {
    let mut statement = conn
        .prepare(
            "SELECT c.id, c.name, c.folder,
                    COUNT(r.id),
                    COALESCE(SUM(r.favorite), 0)
             FROM image_collections c
             LEFT JOIN image_records r ON r.collection_id = c.id
             GROUP BY c.id
             ORDER BY c.created_at ASC, c.id ASC",
        )
        .map_err(|e| format!("查询散图区失败: {e}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok(LooseCollection {
                id: row.get(0)?,
                name: row.get(1)?,
                folder: row.get(2)?,
                count: row.get(3)?,
                favorites: row.get(4)?,
            })
        })
        .map_err(|e| format!("读取散图区失败: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取散图区失败: {e}"))
}

/// 删除区：只删记录，磁盘文件夹原样保留（里面的文件是管理员自己的）。
pub fn delete_collection(conn: &mut Connection, id: i64) -> Result<(), String> {
    let transaction = db::begin_write(conn)?;
    transaction
        .execute("DELETE FROM image_records WHERE collection_id = ?1", [id])
        .map_err(|e| format!("清理散图记录失败: {e}"))?;
    let changed = transaction
        .execute("DELETE FROM image_collections WHERE id = ?1", [id])
        .map_err(|e| format!("删除散图区失败: {e}"))?;
    transaction
        .commit()
        .map_err(|e| format!("提交删除失败: {e}"))?;
    if changed == 0 {
        return Err("散图区不存在".to_string());
    }
    Ok(())
}

/// 扫描绑定文件夹：新增文件登记、缺失检测。增量、幂等。
pub fn scan_collection(
    conn: &mut Connection,
    collection_id: i64,
) -> Result<(i64, i64, i64), String> {
    let (name, folder) = with_collection(conn, collection_id, |name, folder| {
        (name.to_string(), folder.to_string())
    })
    .ok_or("散图区不存在")?;
    let folder_path = PathBuf::from(&folder);
    if !folder_path.is_dir() {
        return Err(format!("文件夹不见了：{folder}（区「{name}」）"));
    }

    let now = crate::fsutil::now_ms();
    let existing: Vec<String> = {
        let mut statement = conn
            .prepare(
                "SELECT path FROM image_records
                 WHERE collection_id = ?1 AND kind = 'file'",
            )
            .map_err(|e| format!("读取已有记录失败: {e}"))?;
        let rows = statement
            .query_map([collection_id], |row| row.get::<_, String>(0))
            .map_err(|e| format!("读取已有记录失败: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("读取已有记录失败: {e}"))?
    };

    let mut disk_paths: Vec<(PathBuf, i64)> = Vec::new();
    let entries =
        fs::read_dir(&folder_path).map_err(|e| format!("读取文件夹失败: {e}"))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() || !is_image_path(&path) {
            continue;
        }
        let size = entry
            .metadata()
            .map(|meta| meta.len() as i64)
            .unwrap_or(0);
        disk_paths.push((path, size));
    }

    let transaction = db::begin_write(conn)?;
    let mut added = 0i64;
    for (index, (path, size)) in disk_paths.iter().enumerate() {
        let path_text = path.to_string_lossy().to_string();
        if existing.iter().any(|record| record == &path_text) {
            continue;
        }
        transaction
            .execute(
                "INSERT OR IGNORE INTO image_records(
                     collection_id, kind, path, file_size, added_at
                 ) VALUES (?1, 'file', ?2, ?3, ?4)",
                rusqlite::params![
                    collection_id,
                    path_text,
                    size,
                    now + index as i64
                ],
            )
            .map_err(|e| format!("登记散图失败: {e}"))?;
        added += 1;
    }
    // 缺失检测：登记过的文件路径在磁盘上不存在 → 标 missing（不静默删记录）。
    let mut missing = 0i64;
    for path in &existing {
        if !Path::new(path).exists() {
            transaction
                .execute(
                    "UPDATE image_records SET missing = 1
                     WHERE collection_id = ?1 AND kind = 'file' AND path = ?2",
                    rusqlite::params![collection_id, path],
                )
                .map_err(|e| format!("标记缺失失败: {e}"))?;
            missing += 1;
        }
    }
    transaction
        .commit()
        .map_err(|e| format!("提交扫描结果失败: {e}"))?;

    let total = (existing.len() as i64) + added;
    Ok((added, missing, total - missing))
}

fn with_collection<R>(
    conn: &Connection,
    collection_id: i64,
    f: impl FnOnce(&str, &str) -> R,
) -> Option<R> {
    let (name, folder): (String, String) = conn
        .query_row(
            "SELECT name, folder FROM image_collections WHERE id = ?1",
            [collection_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok()?;
    Some(f(&name, &folder))
}

/// 列出区里的记录（源页型带画集标题角标）。
pub fn list_images(conn: &Connection, collection_id: i64) -> Result<Vec<LooseImage>, String> {
    let mut statement = conn
        .prepare(
            "SELECT r.id, r.collection_id, r.kind, r.path, r.archive_id, r.page_index,
                    COALESCE(NULLIF(a.title, ''), a.file_name),
                    r.rating, r.favorite, r.file_size, r.missing, r.added_at
             FROM image_records r
             LEFT JOIN archives a ON a.id = r.archive_id
             WHERE r.collection_id = ?1
             ORDER BY r.added_at ASC, r.id ASC",
        )
        .map_err(|e| format!("查询散图记录失败: {e}"))?;
    let rows = statement
        .query_map([collection_id], |row| {
            Ok(LooseImage {
                id: row.get(0)?,
                collection_id: row.get(1)?,
                kind: row.get(2)?,
                path: row.get(3)?,
                archive_id: row.get(4)?,
                page_index: row.get(5)?,
                archive_title: row.get(6)?,
                rating: row.get(7)?,
                favorite: row.get::<_, i64>(8)? != 0,
                file_size: row.get(9)?,
                missing: row.get::<_, i64>(10)? != 0,
                added_at: row.get(11)?,
            })
        })
        .map_err(|e| format!("读取散图记录失败: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取散图记录失败: {e}"))
}

pub fn set_rating(conn: &Connection, id: i64, rating: i64) -> Result<(), String> {
    conn.execute(
        "UPDATE image_records SET rating = ?1 WHERE id = ?2",
        rusqlite::params![rating.clamp(0, 100), id],
    )
    .map_err(|e| format!("保存评分失败: {e}"))?;
    Ok(())
}

pub fn set_favorite(conn: &Connection, id: i64, favorite: bool) -> Result<(), String> {
    conn.execute(
        "UPDATE image_records SET favorite = ?1 WHERE id = ?2",
        rusqlite::params![favorite as i64, id],
    )
    .map_err(|e| format!("保存收藏标记失败: {e}"))?;
    Ok(())
}

/// 移除记录。`delete_file` 仅对 file 型生效（且只删登记过的那个文件，
/// 路径以库记录为准，防构造参数删任意文件）。
pub fn remove_image(conn: &Connection, id: i64, delete_file: bool) -> Result<(), String> {
    let (kind, path): (String, Option<String>) = conn
        .query_row(
            "SELECT kind, path FROM image_records WHERE id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|_| "记录不存在".to_string())?;
    if delete_file && kind == "file" {
        if let Some(path) = path {
            fs::remove_file(&path).map_err(|e| format!("删除文件失败: {e}"))?;
        }
    }
    conn.execute("DELETE FROM image_records WHERE id = ?1", [id])
        .map_err(|e| format!("移除记录失败: {e}"))?;
    Ok(())
}

/// 从画廊收集一页：默认指向源页（archive_id + page_index），零空间。
/// 已在区内则返回已有记录（幂等）。
pub fn collect_archive_page(
    conn: &Connection,
    collection_id: i64,
    archive_id: i64,
    page_index: i64,
) -> Result<LooseImage, String> {
    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM image_records
             WHERE collection_id = ?1 AND kind = 'archive_page'
               AND archive_id = ?2 AND page_index = ?3",
            rusqlite::params![collection_id, archive_id, page_index],
            |row| row.get(0),
        )
        .ok();
    if let Some(id) = existing {
        return list_images_by_id(conn, id).ok_or_else(|| "记录读取失败".to_string());
    }

    // 画集必须存在且未回收；页号在范围内。
    let (title, page_count): (String, i64) = conn
        .query_row(
            "SELECT COALESCE(NULLIF(title, ''), file_name), page_count
             FROM archives WHERE id = ?1 AND deleted_at IS NULL",
            [archive_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|_| "画集不存在或已在回收站".to_string())?;
    if page_count <= 0 || page_index < 0 || page_index >= page_count {
        return Err("页号超出范围".to_string());
    }
    let now = crate::fsutil::now_ms();
    conn.execute(
        "INSERT INTO image_records(
             collection_id, kind, archive_id, page_index, added_at
         ) VALUES (?1, 'archive_page', ?2, ?3, ?4)",
        rusqlite::params![collection_id, archive_id, page_index, now],
    )
    .map_err(|e| format!("收藏失败: {e}"))?;
    list_images_by_id(conn, conn.last_insert_rowid()).ok_or_else(|| "记录读取失败".to_string())
}

/// 收集时选择「提取到文件夹」：把源页字节写成绑定文件夹里的真实文件。
pub fn extract_page(
    conn: &mut Connection,
    root: &Path,
    collection_id: i64,
    archive_id: i64,
    page_index: i64,
) -> Result<LooseImage, String> {
    let (folder, title, page_count): (String, String, i64) = conn
        .query_row(
            "SELECT c.folder, COALESCE(NULLIF(a.title, ''), a.file_name), a.page_count
             FROM image_collections c
             JOIN archives a ON a.id = ?2
             WHERE c.id = ?1 AND a.deleted_at IS NULL",
            rusqlite::params![collection_id, archive_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|_| "散图区或画集不存在".to_string())?;
    if page_count <= 0 || page_index < 0 || page_index >= page_count {
        return Err("页号超出范围".to_string());
    }

    let rel_path: String = conn
        .query_row(
            "SELECT rel_path FROM archives WHERE id = ?1",
            [archive_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("读取画集路径失败: {e}"))?;
    let archive_path = root.join(rel_path);
    let (mime, bytes) =
        reader::read_page_bytes(&archive_path, page_index)?;
    let ext = mime_to_ext(&mime);
    let base = crate::d2::sanitize_file_name(&format!("{title}-P{}", page_index + 1));

    let folder_path = PathBuf::from(&folder);
    let mut target = folder_path.join(format!("{base}.{ext}"));
    let mut attempt = 2;
    while target.exists() {
        target = folder_path.join(format!("{base}-{attempt}.{ext}"));
        attempt += 1;
    }
    fs::create_dir_all(&folder_path).map_err(|e| format!("文件夹不见了: {e}"))?;
    let mut file = fs::File::create(&target).map_err(|e| format!("写文件失败: {e}"))?;
    file.write_all(&bytes)
        .map_err(|e| format!("写文件失败: {e}"))?;

    let now = crate::fsutil::now_ms();
    conn.execute(
        "INSERT OR IGNORE INTO image_records(
             collection_id, kind, path, file_size, added_at
         ) VALUES (?1, 'file', ?2, ?3, ?4)",
        rusqlite::params![
            collection_id,
            target.to_string_lossy(),
            bytes.len() as i64,
            now
        ],
    )
    .map_err(|e| format!("登记提取结果失败: {e}"))?;
    list_images_by_id(conn, conn.last_insert_rowid()).ok_or_else(|| "记录读取失败".to_string())
}

fn list_images_by_id(conn: &Connection, id: i64) -> Option<LooseImage> {
    list_images_by_collection_any(conn)
        .into_iter()
        .find(|image| image.id == id)
}

/// 单条记录查询（list_images 按区过滤，这里补一个按 id 的）。
fn list_images_by_collection_any(conn: &Connection) -> Vec<LooseImage> {
    let Ok(mut statement) = conn.prepare(
        "SELECT r.id, r.collection_id, r.kind, r.path, r.archive_id, r.page_index,
                COALESCE(NULLIF(a.title, ''), a.file_name),
                r.rating, r.favorite, r.file_size, r.missing, r.added_at
         FROM image_records r
         LEFT JOIN archives a ON a.id = r.archive_id",
    ) else {
        return Vec::new();
    };
    let Ok(rows) = statement.query_map([], |row| {
        Ok(LooseImage {
            id: row.get(0)?,
            collection_id: row.get(1)?,
            kind: row.get(2)?,
            path: row.get(3)?,
            archive_id: row.get(4)?,
            page_index: row.get(5)?,
            archive_title: row.get(6)?,
            rating: row.get(7)?,
            favorite: row.get::<_, i64>(8)? != 0,
            file_size: row.get(9)?,
            missing: row.get::<_, i64>(10)? != 0,
            added_at: row.get(11)?,
        })
    }) else {
        return Vec::new();
    };
    rows.filter_map(Result::ok).collect()
}

fn mime_to_ext(mime: &str) -> String {
    match mime {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/avif" => "avif",
        "image/bmp" => "bmp",
        _ => "jpg",
    }
    .to_string()
}

/// `loose://img/<id>` / `loose://thumb/<id>` 的记录解析：返回
/// （kind, 路径, rel_path, archive_id, page_index）。库未打开/记录缺失返回 None。
pub struct LooseRecord {
    pub kind: String,
    pub path: Option<String>,
    pub rel_path: Option<String>,
    pub archive_id: Option<i64>,
    pub page_index: Option<i64>,
}

pub fn fetch_record(db_path: &Path, id: i64) -> Option<LooseRecord> {
    let connection = Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .ok()?;
    connection
        .query_row(
            "SELECT r.kind, r.path, a.rel_path, r.archive_id, r.page_index
             FROM image_records r
             LEFT JOIN archives a ON a.id = r.archive_id
             WHERE r.id = ?1",
            [id],
            |row| {
                Ok(LooseRecord {
                    kind: row.get(0)?,
                    path: row.get(1)?,
                    rel_path: row.get(2)?,
                    archive_id: row.get(3)?,
                    page_index: row.get(4)?,
                })
            },
        )
        .ok()
}

/// `loose://` 协议服务入口（lib.rs 注册）：
/// `/img/<id>` 全图、`/thumb/<id>` 缩略图（懒生成进缓存）。
pub fn serve(
    app: &AppHandle,
    request: tauri::http::Request<Vec<u8>>,
) -> tauri::http::Response<Vec<u8>> {
    use tauri::http::StatusCode;

    let not_found = |message: &str| {
        tauri::http::Response::builder()
            .status(StatusCode::NOT_FOUND)
            .header("Content-Type", "text/plain; charset=utf-8")
            .body(message.as_bytes().to_vec())
            .unwrap_or_else(|_| tauri::http::Response::new(Vec::new()))
    };

    let Some(state) = app.try_state::<crate::AppState>() else {
        return not_found("库未打开");
    };
    let Some((root, db_path)) = crate::library_root_and_db(&state) else {
        return not_found("库未打开");
    };

    let path = request.uri().path().trim_start_matches('/').to_string();
    let (want_thumb, id) = if let Some(rest) = path.strip_prefix("img/") {
        (false, rest)
    } else if let Some(rest) = path.strip_prefix("thumb/") {
        (true, rest)
    } else {
        return not_found("请求路径无效");
    };
    let Ok(record_id) = id.parse::<i64>() else {
        return not_found("请求路径无效");
    };

    let Some(record) = fetch_record(&db_path, record_id) else {
        return not_found("散图记录不存在");
    };

    // 按记录类型取字节：file 直接读磁盘；archive_page 解包源页（不做缓存，查看频次低）。
    let (bytes, mime): (Vec<u8>, &'static str) = match record.kind.as_str() {
        "file" => {
            let Some(path) = &record.path else {
                return not_found("记录缺路径");
            };
            let ext = Path::new(path)
                .extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or("jpg");
            let mime = mime_for_ext(ext);
            match fs::read(path) {
                Ok(bytes) => (bytes, mime),
                Err(_) => return not_found("文件已被移动或删除"),
            }
        }
        "archive_page" => {
            let (Some(rel_path), Some(archive_id), Some(page_index)) =
                (record.rel_path.clone(), record.archive_id, record.page_index)
            else {
                return not_found("记录缺源页信息");
            };
            match reader::read_page_bytes(&root.join(rel_path), page_index) {
                Ok((mime, bytes)) => (bytes, mime_for_ext(&mime)),
                Err(_) => return not_found("源页读取失败"),
            }
        }
        _ => return not_found("未知记录类型"),
    };

    if !want_thumb {
        // 原图：按内容带版本，文件没变就让 webview 自己缓存。
        let version = crate::fsutil::now_ms();
        return tauri::http::Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", mime)
            .header("Content-Length", bytes.len())
            .header("Cache-Control", "private, max-age=3600")
            .body(bytes)
            .unwrap_or_else(|_| tauri::http::Response::new(Vec::new()));
    }

    // 缩略图：懒生成 + 缓存（按内容指纹寻址，重扫不重算）。
    let cache_dir = match app.path().app_cache_dir() {
        Ok(dir) => dir.join("loose-thumbs"),
        Err(_) => return not_found("缓存目录不可用"),
    };
    let fingerprint = blake3::hash(
        format!("{record_id}:{}:{}", bytes.len(), record.kind).as_bytes(),
    );
    let cache_path = cache_dir.join(format!("{}.jpg", fingerprint.to_hex()));
    let thumb_bytes = match fs::read(&cache_path) {
        Ok(bytes) => bytes,
        Err(_) => {
            let decoded = match image::load_from_memory(&bytes) {
                Ok(decoded) => decoded,
                Err(_) => return not_found("图片解码失败"),
            };
            let thumb = decoded.thumbnail(THUMB_MAX_EDGE, THUMB_MAX_EDGE);
            let mut buffer = std::io::Cursor::new(Vec::new());
            let encoded = thumb.write_to(&mut buffer, image::ImageFormat::Jpeg);
            match encoded {
                Ok(()) => {
                    let _ = fs::create_dir_all(&cache_dir);
                    let _ = fs::File::create(&cache_path)
                        .and_then(|mut file| file.write_all(buffer.get_ref()));
                    buffer.into_inner()
                }
                Err(_) => return not_found("缩略图生成失败"),
            }
        }
    };

    tauri::http::Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "image/jpeg")
        .header("Content-Length", thumb_bytes.len())
        .header("Cache-Control", "private, max-age=86400")
        .body(thumb_bytes)
        .unwrap_or_else(|_| tauri::http::Response::new(Vec::new()))
}

// ── Tauri 命令（散图管理 UI 用）──

#[tauri::command]
pub async fn loose_list_collections(
    state: tauri::State<'_, crate::AppState>,
) -> Result<Vec<LooseCollection>, String> {
    crate::run_blocking(state, move |state| {
        crate::with_library(state, |library| list_collections(&library.conn))
    })
    .await
}

#[tauri::command]
pub async fn loose_create_collection(
    state: tauri::State<'_, crate::AppState>,
    name: String,
    folder: Option<String>,
) -> Result<LooseCollection, String> {
    crate::run_blocking(state, move |state| {
        crate::with_library_mut(state, |library| {
            let root = library
                .root
                .parent()
                .map(|parent| parent.join("散图"))
                .unwrap_or_else(|| library.root.clone());
            create_collection(
                &mut library.conn,
                &name,
                folder.as_deref().map(Path::new),
                &root,
            )
        })
    })
    .await
}

#[tauri::command]
pub async fn loose_delete_collection(
    state: tauri::State<'_, crate::AppState>,
    collection_id: i64,
) -> Result<(), String> {
    crate::run_blocking(state, move |state| {
        crate::with_library_mut(state, |library| {
            delete_collection(&mut library.conn, collection_id)
        })
    })
    .await
}

#[tauri::command]
pub async fn loose_scan_collection(
    state: tauri::State<'_, crate::AppState>,
    collection_id: i64,
) -> Result<(i64, i64, i64), String> {
    crate::run_blocking(state, move |state| {
        crate::with_library_mut(state, |library| {
            scan_collection(&mut library.conn, collection_id)
        })
    })
    .await
}

#[tauri::command]
pub async fn loose_list_images(
    state: tauri::State<'_, crate::AppState>,
    collection_id: i64,
) -> Result<Vec<LooseImage>, String> {
    crate::run_blocking(state, move |state| {
        crate::with_library(state, |library| list_images(&library.conn, collection_id))
    })
    .await
}

#[tauri::command]
pub async fn loose_set_rating(
    state: tauri::State<'_, crate::AppState>,
    id: i64,
    rating: i64,
) -> Result<(), String> {
    crate::run_blocking(state, move |state| {
        crate::with_library(state, |library| set_rating(&library.conn, id, rating))
    })
    .await
}

#[tauri::command]
pub async fn loose_set_favorite(
    state: tauri::State<'_, crate::AppState>,
    id: i64,
    favorite: bool,
) -> Result<(), String> {
    crate::run_blocking(state, move |state| {
        crate::with_library(state, |library| set_favorite(&library.conn, id, favorite))
    })
    .await
}

#[tauri::command]
pub async fn loose_remove_image(
    state: tauri::State<'_, crate::AppState>,
    id: i64,
    delete_file: Option<bool>,
) -> Result<(), String> {
    crate::run_blocking(state, move |state| {
        crate::with_library(state, |library| {
            remove_image(&library.conn, id, delete_file.unwrap_or(false))
        })
    })
    .await
}

#[tauri::command]
pub async fn loose_collect_page(
    state: tauri::State<'_, crate::AppState>,
    collection_id: i64,
    archive_id: i64,
    page_index: i64,
) -> Result<LooseImage, String> {
    crate::run_blocking(state, move |state| {
        crate::with_library(state, |library| {
            collect_archive_page(&library.conn, collection_id, archive_id, page_index)
        })
    })
    .await
}

#[tauri::command]
pub async fn loose_extract_page(
    state: tauri::State<'_, crate::AppState>,
    collection_id: i64,
    archive_id: i64,
    page_index: i64,
) -> Result<LooseImage, String> {
    crate::run_blocking(state, move |state| {
        crate::with_library_mut(state, |library| {
            extract_page(
                &mut library.conn,
                &library.root,
                collection_id,
                archive_id,
                page_index,
            )
        })
    })
    .await
}

/// 在资源管理器里打开散图文件夹（管理员复制粘贴图片用）。
#[tauri::command]
pub fn loose_open_folder(folder: String) -> Result<(), String> {
    std::process::Command::new("explorer")
        .arg(folder)
        .spawn()
        .map_err(|e| format!("打开文件夹失败: {e}"))?;
    Ok(())
}
