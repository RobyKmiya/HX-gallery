use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{
    Connection, ErrorCode, OptionalExtension, Transaction, TransactionBehavior, params,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::fsutil;
use crate::models::{
    Collection, ReaderBookmark, Tag, TagCategory, TagEdge, TagGraph, TagTranslationInput,
    TrashedArchive,
};
use crate::translate;

const SCHEMA_VERSION: i64 = 7;

/// 并发写等待上限（毫秒）。桌面、CLI 与导入线程可能同时写同一个 WAL 库，
/// 原来 5 秒在一次大批量导入期间很容易耗尽，导致部分文件写入失败。
pub const BUSY_TIMEOUT_MS: u32 = 30_000;

/// 单条 `IN (...)` 绑定的参数个数上限。SQLite 默认变量上限为 32766，
/// 这里取更小的批次，既避开上限也避免超长语句。
const IN_CHUNK: usize = 400;

fn is_busy(error: &rusqlite::Error) -> bool {
    matches!(
        error.sqlite_error_code(),
        Some(ErrorCode::DatabaseBusy) | Some(ErrorCode::DatabaseLocked)
    )
}

/// 以 `BEGIN IMMEDIATE` 开始写事务，并对「数据库被占用」做指数退避重试。
///
/// 用 IMMEDIATE 而不是默认的 DEFERRED：进入事务时立刻拿写锁，拿不到可以安全重试；
/// 否则会在读完数据、执行第一条写语句时才报 busy，此时已无法重试。
pub fn begin_write(conn: &Connection) -> Result<Transaction<'_>, String> {
    let mut delay = std::time::Duration::from_millis(50);
    let mut last = String::from("无法开始写事务");
    for attempt in 0..6 {
        match Transaction::new_unchecked(conn, TransactionBehavior::Immediate) {
            Ok(transaction) => return Ok(transaction),
            Err(error) => {
                last = format!("无法开始写事务: {error}");
                if !is_busy(&error) || attempt == 5 {
                    break;
                }
                std::thread::sleep(delay);
                delay = (delay * 2).min(std::time::Duration::from_millis(800));
            }
        }
    }
    Err(last)
}

const FULL_TEXT_SCHEMA: &str = r#"
    CREATE VIRTUAL TABLE IF NOT EXISTS archives_fts USING fts5(
        title,
        file_name,
        note,
        content='archives',
        content_rowid='id',
        tokenize='trigram'
    );

    CREATE TRIGGER IF NOT EXISTS archives_fts_ai AFTER INSERT ON archives BEGIN
        INSERT INTO archives_fts(rowid, title, file_name, note)
        VALUES (new.id, new.title, new.file_name, new.note);
    END;

    CREATE TRIGGER IF NOT EXISTS archives_fts_ad AFTER DELETE ON archives BEGIN
        INSERT INTO archives_fts(archives_fts, rowid, title, file_name, note)
        VALUES ('delete', old.id, old.title, old.file_name, old.note);
    END;

    CREATE TRIGGER IF NOT EXISTS archives_fts_au AFTER UPDATE OF title, file_name, note ON archives BEGIN
        INSERT INTO archives_fts(archives_fts, rowid, title, file_name, note)
        VALUES ('delete', old.id, old.title, old.file_name, old.note);
        INSERT INTO archives_fts(rowid, title, file_name, note)
        VALUES (new.id, new.title, new.file_name, new.note);
    END;
"#;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryMeta {
    pub library_id: String,
    pub name: String,
    pub created_at: i64,
    pub schema_version: i64,
}

pub struct LibraryContext {
    pub root: PathBuf,
    pub library_id: String,
    pub library_name: String,
    pub conn: Connection,
}

#[derive(Debug, Clone)]
pub struct ArchiveSource {
    pub rel_path: String,
    pub page_count: i64,
    pub last_page: i64,
    pub missing: bool,
}

pub fn open_library(root: &Path) -> Result<LibraryContext, String> {
    let root = std::path::absolute(root).map_err(|e| format!("无法解析库目录: {e}"))?;
    fs::create_dir_all(&root).map_err(|e| format!("无法创建库目录: {e}"))?;

    let gallery_dir = root.join(".gallery");
    fs::create_dir_all(&gallery_dir).map_err(|e| format!("无法创建 .gallery 目录: {e}"))?;

    let mut meta = load_or_create_meta(&root, &gallery_dir)?;
    let db_path = gallery_dir.join("library.db");
    let conn = open_connection(&db_path)?;

    migrate(&conn, &gallery_dir)?;
    sync_meta_schema(&gallery_dir, &mut meta)?;

    Ok(LibraryContext {
        root,
        library_id: meta.library_id,
        library_name: meta.name,
        conn,
    })
}

/// 打开一个已完成迁移的库连接，<b>不</b>执行迁移与建表 DDL。
///
/// 供导入、扫描、CLI 子命令等后台任务使用：库结构已由主连接在打开库时升级完毕，
/// 后台任务再跑一遍 `CREATE VIRTUAL TABLE` / `CREATE TRIGGER` 会写 `sqlite_master`、
/// 抢写锁，导致多进程并发启动时随机 `database is locked`。
pub fn open_worker_connection(root: &Path) -> Result<Connection, String> {
    let db_path = root.join(".gallery").join("library.db");
    if !db_path.exists() {
        return Err(format!(
            "{} 还不是已初始化的画廊库，请先在应用中打开该目录",
            root.to_string_lossy()
        ));
    }
    open_connection(&db_path)
}

fn open_connection(db_path: &Path) -> Result<Connection, String> {
    let conn = Connection::open(db_path).map_err(|e| format!("无法打开数据库: {e}"))?;
    configure_connection(&conn)?;
    Ok(conn)
}

fn configure_connection(conn: &Connection) -> Result<(), String> {
    let mode: String = conn
        .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
        .map_err(|e| format!("无法启用 WAL: {e}"))?;
    // SQLite 在无法切换日志模式时不报错，只返回当前模式字符串。丢弃返回值会让
    // 后续所有「WAL 支持并发读」的假设悄悄失效。
    if !mode.eq_ignore_ascii_case("wal") {
        return Err(format!(
            "数据库未能进入 WAL 模式（当前为 {mode}），可能有其它进程正在写入"
        ));
    }

    // synchronous = NORMAL 在 WAL 下只在 checkpoint 时 fsync，写入吞吐提升明显且不会损坏库；
    // wal_autocheckpoint 让 WAL 随写入自动回收，避免大批量导入后无限膨胀。
    conn.execute_batch(&format!(
        "PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = {BUSY_TIMEOUT_MS};
         PRAGMA synchronous = NORMAL;
         PRAGMA wal_autocheckpoint = 1000;"
    ))
    .map_err(|e| format!("无法配置数据库连接: {e}"))
}

fn sync_meta_schema(gallery_dir: &Path, meta: &mut LibraryMeta) -> Result<(), String> {
    if meta.schema_version == SCHEMA_VERSION {
        return Ok(());
    }

    meta.schema_version = SCHEMA_VERSION;
    let path = gallery_dir.join("library.json");
    let raw =
        serde_json::to_string_pretty(meta).map_err(|e| format!("无法生成 library.json: {e}"))?;
    fs::write(&path, raw).map_err(|e| format!("无法更新 library.json: {e}"))
}

/// `library_id` 会被拼进封面缓存目录（`cache/covers/<library_id>`）。
/// 手工编辑或被篡改的 `library.json` 里若含 `..` 或路径分隔符，
/// `create_dir_all` 就会在缓存目录之外创建目录并把封面写到那里。
fn is_safe_library_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.chars().all(|ch| ch.is_ascii_hexdigit() || ch == '-')
}

fn load_or_create_meta(root: &Path, gallery_dir: &Path) -> Result<LibraryMeta, String> {
    let path = gallery_dir.join("library.json");
    if path.exists() {
        let raw = fs::read_to_string(&path).map_err(|e| format!("无法读取 library.json: {e}"))?;
        let mut meta: LibraryMeta =
            serde_json::from_str(&raw).map_err(|e| format!("library.json 格式错误: {e}"))?;

        if !is_safe_library_id(&meta.library_id) {
            // 不因此拒绝打开库（否则一个手滑的字段就让库永久打不开），
            // 改为换成新的 UUID 并落盘；旧封面目录会失联，下次扫描自动重建。
            let fresh = Uuid::new_v4().to_string();
            eprintln!(
                "library.json 的 library_id 不合法（{:?}），已重新生成为 {fresh}",
                meta.library_id
            );
            meta.library_id = fresh;
            let raw = serde_json::to_string_pretty(&meta)
                .map_err(|e| format!("无法生成 library.json: {e}"))?;
            fs::write(&path, raw).map_err(|e| format!("无法更新 library.json: {e}"))?;
        }

        return Ok(meta);
    }

    let name = root
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "本地画廊".to_string());

    let meta = LibraryMeta {
        library_id: Uuid::new_v4().to_string(),
        name,
        created_at: fsutil::now_ms(),
        schema_version: SCHEMA_VERSION,
    };

    let raw = serde_json::to_string_pretty(&meta)
        .map_err(|e| format!("无法生成 library.json: {e}"))?;
    fs::write(&path, raw).map_err(|e| format!("无法写入 library.json: {e}"))?;

    Ok(meta)
}

fn migrate(conn: &Connection, gallery_dir: &Path) -> Result<(), String> {
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|e| format!("无法读取数据库版本: {e}"))?;

    if version > SCHEMA_VERSION {
        return Err(format!(
            "数据库版本 {version} 高于当前程序支持的版本 {SCHEMA_VERSION}"
        ));
    }

    if version == 0 {
        // 新库：建表 + 写版本号放在同一事务里，避免中断后留下「有表但 user_version 仍为 0」的库。
        let transaction = begin_write(conn)?;
        create_schema(&transaction)?;
        transaction
            .execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))
            .map_err(|e| format!("写入数据库版本失败: {e}"))?;
        transaction
            .commit()
            .map_err(|e| format!("提交建库事务失败: {e}"))?;
    } else if version < SCHEMA_VERSION {
        // VACUUM INTO 不能在事务内执行，备份必须留在事务外。
        backup_database(conn, gallery_dir, version)?;
        let transaction = begin_write(conn)?;
        upgrade_schema(&transaction)?;
        transaction
            .execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))
            .map_err(|e| format!("写入数据库版本失败: {e}"))?;
        transaction
            .commit()
            .map_err(|e| format!("提交升级事务失败: {e}"))?;
    }

    ensure_full_text(conn)
}

fn create_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS archives (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            rel_path TEXT NOT NULL UNIQUE,
            file_name TEXT NOT NULL,
            ext TEXT NOT NULL,
            file_size INTEGER NOT NULL,
            file_mtime INTEGER NOT NULL,
            page_count INTEGER NOT NULL DEFAULT 0,
            title TEXT,
            rating INTEGER NOT NULL DEFAULT 60 CHECK (rating BETWEEN 0 AND 100),
            note TEXT,
            cover_key TEXT,
            content_hash TEXT,
            added_at INTEGER NOT NULL,
            last_read_at INTEGER,
            last_page INTEGER NOT NULL DEFAULT 0,
            missing INTEGER NOT NULL DEFAULT 0,
            deleted_at INTEGER,
            error TEXT
        );

        CREATE UNIQUE INDEX IF NOT EXISTS idx_archives_hash
            ON archives(content_hash) WHERE content_hash IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_archives_added_at ON archives(added_at);
        CREATE INDEX IF NOT EXISTS idx_archives_file_mtime ON archives(file_mtime);
        CREATE INDEX IF NOT EXISTS idx_archives_rating ON archives(rating);
        CREATE INDEX IF NOT EXISTS idx_archives_file_size ON archives(file_size);
        CREATE INDEX IF NOT EXISTS idx_archives_missing ON archives(missing);
        CREATE INDEX IF NOT EXISTS idx_archives_deleted_at ON archives(deleted_at);
        CREATE INDEX IF NOT EXISTS idx_archives_title ON archives(title COLLATE NOCASE);

        CREATE TABLE IF NOT EXISTS collections (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL UNIQUE,
            is_default INTEGER NOT NULL DEFAULT 0,
            created_at INTEGER NOT NULL,
            sort_order INTEGER NOT NULL DEFAULT 0,
            cover_archive_id INTEGER REFERENCES archives(id) ON DELETE SET NULL,
            deleted_at INTEGER
        );

        CREATE UNIQUE INDEX IF NOT EXISTS idx_collections_default
            ON collections(is_default) WHERE is_default = 1;
        CREATE INDEX IF NOT EXISTS idx_collections_sort_order
            ON collections(sort_order);
        CREATE INDEX IF NOT EXISTS idx_collections_deleted_at
            ON collections(deleted_at);

        CREATE TABLE IF NOT EXISTS collection_archives (
            collection_id INTEGER NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
            archive_id INTEGER NOT NULL REFERENCES archives(id) ON DELETE CASCADE,
            added_at INTEGER NOT NULL,
            PRIMARY KEY (collection_id, archive_id)
        );

        CREATE INDEX IF NOT EXISTS idx_collection_archives_archive
            ON collection_archives(archive_id);

        CREATE TABLE IF NOT EXISTS tags (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL UNIQUE,
            original_name TEXT NOT NULL DEFAULT '',
            translated_name TEXT,
            translation_source TEXT,
            translated_at INTEGER,
            category TEXT,
            deleted_at INTEGER,
            merged_into INTEGER REFERENCES tags(id) ON DELETE SET NULL
        );

        CREATE INDEX IF NOT EXISTS idx_tags_deleted_at ON tags(deleted_at);
        CREATE INDEX IF NOT EXISTS idx_tags_category ON tags(category);

        CREATE TABLE IF NOT EXISTS tag_categories (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            parent_id INTEGER REFERENCES tag_categories(id) ON DELETE CASCADE,
            path TEXT NOT NULL,
            created_at INTEGER NOT NULL
        );

        CREATE UNIQUE INDEX IF NOT EXISTS idx_tag_categories_path
            ON tag_categories(path COLLATE NOCASE);
        CREATE INDEX IF NOT EXISTS idx_tag_categories_parent
            ON tag_categories(parent_id);

        CREATE TABLE IF NOT EXISTS archive_tags (
            archive_id INTEGER NOT NULL REFERENCES archives(id) ON DELETE CASCADE,
            tag_id INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
            PRIMARY KEY (archive_id, tag_id)
        );

        CREATE INDEX IF NOT EXISTS idx_archive_tags_tag ON archive_tags(tag_id);

        CREATE TABLE IF NOT EXISTS reader_bookmarks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            archive_id INTEGER NOT NULL REFERENCES archives(id) ON DELETE CASCADE,
            page INTEGER NOT NULL CHECK (page > 0),
            created_at INTEGER NOT NULL,
            UNIQUE (archive_id, page)
        );

        CREATE INDEX IF NOT EXISTS idx_reader_bookmarks_archive
            ON reader_bookmarks(archive_id, page);
        "#,
    )
    .map_err(|e| format!("初始化数据库失败: {e}"))?;

    conn.execute(
        "INSERT INTO collections(name, is_default, created_at, sort_order)
         SELECT '收藏', 1, ?1, 0
         WHERE NOT EXISTS (SELECT 1 FROM collections WHERE is_default = 1)",
        params![fsutil::now_ms()],
    )
    .map_err(|e| format!("创建默认收藏夹失败: {e}"))?;

    Ok(())
}

fn backup_database(conn: &Connection, gallery_dir: &Path, version: i64) -> Result<(), String> {
    let db_path = gallery_dir.join("library.db");
    if !db_path.exists() {
        return Ok(());
    }

    let backup_dir = gallery_dir.join("backups");
    fs::create_dir_all(&backup_dir).map_err(|e| format!("无法创建迁移备份目录: {e}"))?;
    let backup_path = backup_dir.join(format!("library-v{version}-{}.db", fsutil::now_ms()));

    // VACUUM INTO 由当前连接生成一致性快照，包含 WAL 中尚未 checkpoint 的已提交事务。
    // 原先先 checkpoint 再 fs::copy 主库的做法，在 checkpoint 失败（另一实例持有连接）
    // 时会静默丢掉最近写入，且 checkpoint 的返回值被丢弃。
    let target = backup_path.to_string_lossy().to_string();
    conn.execute("VACUUM INTO ?1", rusqlite::params![target])
        .map_err(|e| {
            format!(
                "迁移前备份失败，已中止升级（{}）: {e}",
                backup_path.to_string_lossy()
            )
        })?;

    let size = fs::metadata(&backup_path)
        .map(|meta| meta.len())
        .unwrap_or(0);
    if size == 0 {
        let _ = fs::remove_file(&backup_path);
        return Err(format!(
            "迁移前备份为空，已中止升级（{}）",
            backup_path.to_string_lossy()
        ));
    }

    eprintln!(
        "database v{version} backed up to {} ({size} bytes)",
        backup_path.to_string_lossy()
    );
    Ok(())
}

fn upgrade_schema(conn: &Connection) -> Result<(), String> {
    add_column_if_missing(conn, "tags", "translated_name", "TEXT")?;
    add_column_if_missing(conn, "tags", "original_name", "TEXT NOT NULL DEFAULT ''")?;
    add_column_if_missing(conn, "tags", "translation_source", "TEXT")?;
    add_column_if_missing(conn, "tags", "translated_at", "INTEGER")?;
    add_column_if_missing(conn, "tags", "deleted_at", "INTEGER")?;
    add_column_if_missing(conn, "tags", "merged_into", "INTEGER")?;
    add_column_if_missing(conn, "collections", "sort_order", "INTEGER NOT NULL DEFAULT 0")?;
    add_column_if_missing(
        conn,
        "collections",
        "cover_archive_id",
        "INTEGER REFERENCES archives(id) ON DELETE SET NULL",
    )?;
    add_column_if_missing(conn, "collections", "deleted_at", "INTEGER")?;
    add_column_if_missing(conn, "archives", "deleted_at", "INTEGER")?;

    conn.execute(
        "UPDATE tags SET original_name = name
         WHERE original_name IS NULL OR original_name = ''",
        [],
    )
    .map_err(|e| format!("回填标签原始名失败: {e}"))?;
    conn.execute(
        "UPDATE tags SET translation_source = 'legacy'
         WHERE translated_name IS NOT NULL AND translated_name <> ''
           AND translation_source IS NULL",
        [],
    )
    .map_err(|e| format!("回填翻译来源失败: {e}"))?;
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_tags_deleted_at ON tags(deleted_at);
         CREATE INDEX IF NOT EXISTS idx_tags_category ON tags(category);
         CREATE INDEX IF NOT EXISTS idx_collections_sort_order ON collections(sort_order);
         CREATE INDEX IF NOT EXISTS idx_collections_deleted_at ON collections(deleted_at);
         CREATE INDEX IF NOT EXISTS idx_archives_deleted_at ON archives(deleted_at);
         CREATE INDEX IF NOT EXISTS idx_archives_title ON archives(title COLLATE NOCASE);

         CREATE TABLE IF NOT EXISTS tag_categories (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             name TEXT NOT NULL,
             parent_id INTEGER REFERENCES tag_categories(id) ON DELETE CASCADE,
             path TEXT NOT NULL,
             created_at INTEGER NOT NULL
         );
         CREATE UNIQUE INDEX IF NOT EXISTS idx_tag_categories_path
             ON tag_categories(path COLLATE NOCASE);
         CREATE INDEX IF NOT EXISTS idx_tag_categories_parent
             ON tag_categories(parent_id);

         CREATE TABLE IF NOT EXISTS reader_bookmarks (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             archive_id INTEGER NOT NULL REFERENCES archives(id) ON DELETE CASCADE,
             page INTEGER NOT NULL CHECK (page > 0),
             created_at INTEGER NOT NULL,
             UNIQUE (archive_id, page)
         );
         CREATE INDEX IF NOT EXISTS idx_reader_bookmarks_archive
             ON reader_bookmarks(archive_id, page);",
    )
    .map_err(|e| format!("创建索引失败: {e}"))?;

    backfill_translations(conn)?;
    backfill_tag_categories(conn)?;
    Ok(())
}

fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<(), String> {
    if has_column(conn, table, column)? {
        return Ok(());
    }
    conn.execute_batch(&format!(
        "ALTER TABLE {table} ADD COLUMN {column} {definition};"
    ))
    .map_err(|e| format!("升级 {table}.{column} 失败: {e}"))
}

fn ensure_full_text(conn: &Connection) -> Result<(), String> {
    let table_exists = object_exists(conn, "table", "archives_fts")?;
    let mut triggers_ok = true;
    for name in ["archives_fts_ai", "archives_fts_ad", "archives_fts_au"] {
        triggers_ok &= object_exists(conn, "trigger", name)?;
    }

    // 原来的实现每次 open_library 都无条件执行这段 DDL：即使全是 IF NOT EXISTS，
    // 只要表/触发器已存在就不会真正新建，但表结构缺失时又必须补建。这里只在缺失时执行。
    if !table_exists || !triggers_ok {
        conn.execute_batch(FULL_TEXT_SCHEMA)
            .map_err(|e| format!("创建全文索引失败: {e}"))?;
        conn.execute_batch("INSERT INTO archives_fts(archives_fts) VALUES('rebuild');")
            .map_err(|e| format!("重建全文索引失败: {e}"))?;
        return Ok(());
    }

    // 外部内容表的索引不会自动跟随 content 表恢复（备份还原、异常退出都可能留下
    // 空索引或半截索引），而此时搜索会静默失效。integrity-check 会逐行比对索引与
    // content 表，不一致时报错，这里据此重建。
    if let Err(error) =
        conn.execute_batch("INSERT INTO archives_fts(archives_fts) VALUES('integrity-check');")
    {
        eprintln!("full-text index check failed ({error}), rebuilding");
        conn.execute_batch("INSERT INTO archives_fts(archives_fts) VALUES('rebuild');")
            .map_err(|e| format!("重建全文索引失败: {e}"))?;
    }

    Ok(())
}

fn object_exists(conn: &Connection, kind: &str, name: &str) -> Result<bool, String> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = ?1 AND name = ?2",
            params![kind, name],
            |row| row.get(0),
        )
        .map_err(|e| format!("检查 {kind} {name} 失败: {e}"))?;
    Ok(count > 0)
}

fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool, String> {
    let mut statement = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|e| format!("读取表结构失败: {e}"))?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| format!("读取表结构失败: {e}"))?;

    for name in rows {
        if name.map_err(|e| format!("读取表结构失败: {e}"))? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn backfill_translations(conn: &Connection) -> Result<(), String> {
    let mut statement = conn
        .prepare("SELECT id, name FROM tags WHERE translated_name IS NULL OR translated_name = ''")
        .map_err(|e| format!("准备标签翻译失败: {e}"))?;

    let rows = statement
        .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)))
        .map_err(|e| format!("读取待翻译标签失败: {e}"))?;

    let pending: Vec<(i64, String)> = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取待翻译标签失败: {e}"))?;

    for (id, name) in pending {
        if let Some(translated) = translate::translate_tag(&name) {
            conn.execute(
                "UPDATE tags SET
                    translated_name = ?1,
                    translation_source = 'dictionary',
                    translated_at = ?2
                 WHERE id = ?3",
                params![translated, fsutil::now_ms(), id],
            )
            .map_err(|e| format!("写入标签翻译失败: {e}"))?;
        }
    }

    Ok(())
}

pub fn stats(conn: &Connection) -> Result<(i64, i64, i64), String> {
    conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(file_size), 0), COALESCE(SUM(missing), 0)
         FROM archives
         WHERE deleted_at IS NULL",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )
    .map_err(|e| format!("读取库统计失败: {e}"))
}

pub fn find_duplicate(conn: &Connection, hash: &str) -> Result<Option<i64>, String> {
    conn.query_row(
        "SELECT id FROM archives WHERE content_hash = ?1 LIMIT 1",
        params![hash],
        |row| row.get(0),
    )
    .optional()
    .map_err(|e| format!("检查重复文件失败: {e}"))
}

pub fn insert_archive(
    conn: &Connection,
    rel_path: &str,
    file_name: &str,
    ext: &str,
    file_size: i64,
    file_mtime: i64,
    content_hash: &str,
    page_count: i64,
    cover_key: Option<&str>,
    error: Option<&str>,
    added_at: i64,
) -> Result<i64, String> {
    let title = Path::new(file_name)
        .file_stem()
        .map(|value| value.to_string_lossy().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| file_name.to_string());

    conn.execute(
        "INSERT INTO archives(
            rel_path, file_name, ext, file_size, file_mtime, page_count, title,
            rating, cover_key, content_hash, added_at, error
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 60, ?8, ?9, ?10, ?11)",
        params![
            rel_path,
            file_name,
            ext,
            file_size,
            file_mtime,
            page_count,
            title,
            cover_key,
            content_hash,
            added_at,
            error
        ],
    )
    .map_err(|e| format!("写入画集记录失败: {e}"))?;

    Ok(conn.last_insert_rowid())
}

#[allow(clippy::too_many_arguments)]
pub fn update_archive_content(
    conn: &Connection,
    id: i64,
    rel_path: &str,
    file_name: &str,
    ext: &str,
    file_size: i64,
    file_mtime: i64,
    content_hash: &str,
    page_count: i64,
    cover_key: Option<&str>,
    error: Option<&str>,
) -> Result<(), String> {
    conn.execute(
        "UPDATE archives SET
            rel_path = ?1,
            file_name = ?2,
            ext = ?3,
            file_size = ?4,
            file_mtime = ?5,
            content_hash = ?6,
            page_count = ?7,
            cover_key = ?8,
            error = ?9,
            missing = 0
         WHERE id = ?10",
        params![
            rel_path,
            file_name,
            ext,
            file_size,
            file_mtime,
            content_hash,
            page_count,
            cover_key,
            error,
            id
        ],
    )
    .map_err(|e| format!("更新画集记录失败: {e}"))?;

    Ok(())
}

pub fn set_rating(conn: &Connection, id: i64, rating: i64) -> Result<(), String> {
    if !(0..=100).contains(&rating) {
        return Err(format!("评分必须在 0 到 100 之间，收到 {rating}"));
    }

    let changed = conn
        .execute(
            "UPDATE archives SET rating = ?1 WHERE id = ?2 AND deleted_at IS NULL",
            params![rating, id],
        )
        .map_err(|e| format!("更新评分失败: {e}"))?;

    if changed == 0 {
        return Err("画集不存在，可能已被移除".to_string());
    }

    Ok(())
}

pub fn set_note(conn: &Connection, id: i64, note: Option<&str>) -> Result<Option<String>, String> {
    let note = note
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            if value.chars().count() > 2000 {
                Err("备注最多 2000 个字符".to_string())
            } else {
                Ok(value.to_string())
            }
        })
        .transpose()?;

    let changed = conn
        .execute(
            "UPDATE archives SET note = ?1 WHERE id = ?2 AND deleted_at IS NULL",
            params![note, id],
        )
        .map_err(|e| format!("更新备注失败: {e}"))?;

    if changed == 0 {
        return Err("画集不存在，可能已被移除".to_string());
    }

    Ok(note)
}

pub fn set_missing(conn: &Connection, id: i64, missing: bool) -> Result<(), String> {
    conn.execute(
        "UPDATE archives SET missing = ?1 WHERE id = ?2",
        params![if missing { 1 } else { 0 }, id],
    )
    .map_err(|e| format!("更新缺失状态失败: {e}"))?;
    Ok(())
}

/// 批量标记缺失。扫描一本书一次 `UPDATE` 会让上千条记录产生上千次提交，
/// 这里分批用一条语句完成，并把整批放进同一个写事务。
/// 返回真正被改成「缺失」的行数（原本已标记的不重复计数）。
pub fn set_missing_many(conn: &Connection, ids: &[i64]) -> Result<usize, String> {
    if ids.is_empty() {
        return Ok(0);
    }

    let transaction = begin_write(conn)?;
    let mut changed = 0usize;
    for chunk in ids.chunks(IN_CHUNK) {
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let sql =
            format!("UPDATE archives SET missing = 1 WHERE missing = 0 AND id IN ({placeholders})");
        changed += transaction
            .execute(&sql, rusqlite::params_from_iter(chunk.iter()))
            .map_err(|e| format!("批量标记缺失失败: {e}"))?;
    }
    transaction
        .commit()
        .map_err(|e| format!("提交缺失标记失败: {e}"))?;

    Ok(changed)
}

/// 彻底删除画集后，检查同一封面文件是否仍被其它记录（含回收站中的记录）引用。
/// 封面按内容哈希命名，正常情况下不会共享，但迁移异常或数据被手工改动时可能共用，
/// 直接删文件会让另一条记录的封面一起丢失。
pub fn cover_key_in_use(conn: &Connection, cover_key: &str) -> Result<bool, String> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM archives WHERE cover_key = ?1",
            params![cover_key],
            |row| row.get(0),
        )
        .map_err(|e| format!("检查封面引用失败: {e}"))?;
    Ok(count > 0)
}

pub fn delete_archive(conn: &Connection, archive_id: i64) -> Result<(), String> {
    let changed = conn
        .execute(
            "UPDATE archives SET deleted_at = ?1
             WHERE id = ?2 AND deleted_at IS NULL",
            params![fsutil::now_ms(), archive_id],
        )
        .map_err(|e| format!("移入回收站失败: {e}"))?;

    if changed == 0 {
        return Err("画集不存在或已在回收站".to_string());
    }

    Ok(())
}

pub fn restore_archive(conn: &Connection, archive_id: i64) -> Result<(), String> {
    let changed = conn
        .execute(
            "UPDATE archives SET deleted_at = NULL
             WHERE id = ?1 AND deleted_at IS NOT NULL",
            params![archive_id],
        )
        .map_err(|e| format!("恢复画集失败: {e}"))?;

    if changed == 0 {
        return Err("回收站里没有这个画集".to_string());
    }

    Ok(())
}

pub fn purge_archive(conn: &Connection, archive_id: i64) -> Result<Option<String>, String> {
    let cover_key: Option<Option<String>> = conn
        .query_row(
            "SELECT cover_key FROM archives
             WHERE id = ?1 AND deleted_at IS NOT NULL",
            params![archive_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("检查回收站画集失败: {e}"))?;

    let cover_key = match cover_key {
        None => return Err("只能彻底删除回收站中的画集".to_string()),
        Some(key) => key,
    };

    conn.execute("DELETE FROM archives WHERE id = ?1", params![archive_id])
        .map_err(|e| format!("彻底删除画集失败: {e}"))?;

    Ok(cover_key)
}

pub fn list_deleted_archives(
    conn: &Connection,
    covers_dir: Option<&Path>,
) -> Result<Vec<TrashedArchive>, String> {
    let mut statement = conn
        .prepare(
            "SELECT a.id, COALESCE(NULLIF(a.title, ''), a.file_name), a.file_name,
                    a.ext, a.file_size, a.page_count, a.last_read_at, a.last_page,
                    a.missing, a.cover_key, a.deleted_at
             FROM archives a
             WHERE a.deleted_at IS NOT NULL
             ORDER BY a.deleted_at DESC, a.id DESC",
        )
        .map_err(|e| format!("准备画集回收站查询失败: {e}"))?;

    let rows = statement
        .query_map([], |row| {
            let cover_key: Option<String> = row.get(9)?;
            let cover_path = match (covers_dir, cover_key) {
                (Some(dir), Some(key)) => Some(dir.join(key).to_string_lossy().to_string()),
                _ => None,
            };
            Ok(TrashedArchive {
                id: row.get(0)?,
                title: row.get(1)?,
                file_name: row.get(2)?,
                ext: row.get(3)?,
                file_size: row.get(4)?,
                page_count: row.get(5)?,
                last_read_at: row.get(6)?,
                last_page: row.get(7)?,
                missing: row.get::<_, i64>(8)? != 0,
                cover_path,
                deleted_at: row.get(10)?,
            })
        })
        .map_err(|e| format!("查询画集回收站失败: {e}"))?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取画集回收站失败: {e}"))
}

pub fn add_to_collection(
    conn: &Connection,
    collection_id: i64,
    archive_id: i64,
) -> Result<(), String> {
    conn.execute(
        "INSERT OR IGNORE INTO collection_archives(collection_id, archive_id, added_at)
         VALUES (?1, ?2, ?3)",
        params![collection_id, archive_id, fsutil::now_ms()],
    )
    .map_err(|e| format!("加入收藏夹失败: {e}"))?;
    Ok(())
}

fn ensure_archive_exists(conn: &Connection, archive_id: i64) -> Result<(), String> {
    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM archives WHERE id = ?1 AND deleted_at IS NULL",
            params![archive_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("检查画集失败: {e}"))?;

    existing
        .map(|_| ())
        .ok_or_else(|| "画集不存在、可能已被移除或在回收站".to_string())
}

fn normalize_label(raw: &str, max_chars: usize, label: &str) -> Result<String, String> {
    let trimmed = raw.trim().trim_start_matches('#').trim();
    if trimmed.is_empty() {
        return Err(format!("{label}不能为空"));
    }
    if trimmed.chars().count() > max_chars {
        return Err(format!("{label}最多 {max_chars} 个字符"));
    }
    Ok(trimmed.to_string())
}

const TAG_SELECT: &str = "t.id, t.name, t.original_name, t.translated_name,
     t.translation_source, t.translated_at, t.category, t.merged_into, t.deleted_at";

fn read_tag(row: &rusqlite::Row<'_>, count_index: usize) -> rusqlite::Result<Tag> {
    let id: i64 = row.get(0)?;
    Ok(Tag {
        id,
        number: id,
        name: row.get(1)?,
        original_name: row.get(2)?,
        translated_name: row.get(3)?,
        translation_source: row.get(4)?,
        translated_at: row.get(5)?,
        category: row.get(6)?,
        merged_into: row.get(7)?,
        deleted_at: row.get(8)?,
        count: row.get(count_index)?,
    })
}

fn tag_by_id(conn: &Connection, tag_id: i64) -> Result<Tag, String> {
    conn.query_row(
        &format!(
            "SELECT {TAG_SELECT},
                    (SELECT COUNT(*) FROM archive_tags x WHERE x.tag_id = t.id)
             FROM tags t WHERE t.id = ?1"
        ),
        params![tag_id],
        |row| read_tag(row, 9),
    )
    .optional()
    .map_err(|e| format!("读取标签失败: {e}"))?
    .ok_or_else(|| "标签不存在，可能已被删除".to_string())
}

pub fn list_tags(conn: &Connection) -> Result<Vec<Tag>, String> {
    let mut statement = conn
        .prepare(&format!(
            "SELECT {TAG_SELECT}, COUNT(a.id)
             FROM tags t
             LEFT JOIN archive_tags at ON at.tag_id = t.id
             LEFT JOIN archives a ON a.id = at.archive_id AND a.deleted_at IS NULL
             WHERE t.deleted_at IS NULL
             GROUP BY t.id
             ORDER BY COUNT(a.id) DESC, t.id ASC"
        ))
        .map_err(|e| format!("准备标签查询失败: {e}"))?;

    let rows = statement
        .query_map([], |row| read_tag(row, 9))
        .map_err(|e| format!("查询标签失败: {e}"))?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取标签失败: {e}"))
}

pub fn list_deleted_tags(conn: &Connection) -> Result<Vec<Tag>, String> {
    let mut statement = conn
        .prepare(&format!(
            "SELECT {TAG_SELECT}, COUNT(a.id)
             FROM tags t
             LEFT JOIN archive_tags at ON at.tag_id = t.id
             LEFT JOIN archives a ON a.id = at.archive_id AND a.deleted_at IS NULL
             WHERE t.deleted_at IS NOT NULL
             GROUP BY t.id
             ORDER BY t.deleted_at DESC, t.id DESC"
        ))
        .map_err(|e| format!("准备回收站查询失败: {e}"))?;

    let rows = statement
        .query_map([], |row| read_tag(row, 9))
        .map_err(|e| format!("查询回收站失败: {e}"))?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取回收站失败: {e}"))
}

pub fn archive_tags(conn: &Connection, archive_id: i64) -> Result<Vec<Tag>, String> {
    let mut statement = conn
        .prepare(&format!(
            "SELECT {TAG_SELECT},
                    (SELECT COUNT(*) FROM archive_tags x WHERE x.tag_id = t.id)
             FROM tags t
             JOIN archive_tags at ON at.tag_id = t.id
             WHERE at.archive_id = ?1 AND t.deleted_at IS NULL
             ORDER BY t.name COLLATE NOCASE"
        ))
        .map_err(|e| format!("准备标签查询失败: {e}"))?;

    let rows = statement
        .query_map(params![archive_id], |row| read_tag(row, 9))
        .map_err(|e| format!("查询画集标签失败: {e}"))?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取画集标签失败: {e}"))
}

pub fn archive_source(conn: &Connection, archive_id: i64) -> Result<ArchiveSource, String> {
    conn.query_row(
        "SELECT rel_path, page_count, last_page, missing FROM archives
         WHERE id = ?1 AND deleted_at IS NULL",
        params![archive_id],
        |row| {
            Ok(ArchiveSource {
                rel_path: row.get(0)?,
                page_count: row.get(1)?,
                last_page: row.get(2)?,
                missing: row.get::<_, i64>(3)? != 0,
            })
        },
    )
    .optional()
    .map_err(|e| format!("查询画集失败: {e}"))?
    .ok_or_else(|| "画集不存在".to_string())
}

pub fn set_reading_progress(
    conn: &Connection,
    archive_id: i64,
    page: i64,
) -> Result<(), String> {
    if page < 1 {
        return Err("页码必须大于 0".to_string());
    }

    let updated = conn
        .execute(
            "UPDATE archives SET last_page = ?1, last_read_at = ?2
             WHERE id = ?3 AND deleted_at IS NULL",
            params![page, fsutil::now_ms(), archive_id],
        )
        .map_err(|e| format!("保存阅读进度失败: {e}"))?;

    if updated == 0 {
        return Err("画集不存在".to_string());
    }

    Ok(())
}

pub fn reader_bookmarks(
    conn: &Connection,
    archive_id: i64,
) -> Result<Vec<ReaderBookmark>, String> {
    let mut statement = conn
        .prepare(
            "SELECT id, archive_id, page, created_at
             FROM reader_bookmarks
             WHERE archive_id = ?1
             ORDER BY page",
        )
        .map_err(|e| format!("准备书签查询失败: {e}"))?;

    let rows = statement
        .query_map(params![archive_id], |row| {
            Ok(ReaderBookmark {
                id: row.get(0)?,
                archive_id: row.get(1)?,
                page: row.get(2)?,
                created_at: row.get(3)?,
            })
        })
        .map_err(|e| format!("查询书签失败: {e}"))?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取书签失败: {e}"))
}

pub fn add_reader_bookmark(
    conn: &Connection,
    archive_id: i64,
    page: i64,
) -> Result<ReaderBookmark, String> {
    if page < 1 {
        return Err("页码必须大于 0".to_string());
    }

    let source = archive_source(conn, archive_id)?;
    if source.page_count > 0 && page > source.page_count {
        return Err(format!(
            "页码超出范围：共 {} 页",
            source.page_count
        ));
    }

    let now = fsutil::now_ms();
    conn.execute(
        "INSERT INTO reader_bookmarks(archive_id, page, created_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(archive_id, page) DO UPDATE SET created_at = excluded.created_at",
        params![archive_id, page, now],
    )
    .map_err(|e| format!("添加书签失败: {e}"))?;

    conn.query_row(
        "SELECT id, archive_id, page, created_at
         FROM reader_bookmarks WHERE archive_id = ?1 AND page = ?2",
        params![archive_id, page],
        |row| {
            Ok(ReaderBookmark {
                id: row.get(0)?,
                archive_id: row.get(1)?,
                page: row.get(2)?,
                created_at: row.get(3)?,
            })
        },
    )
    .map_err(|e| format!("读取新书签失败: {e}"))
}

pub fn remove_reader_bookmark(
    conn: &Connection,
    archive_id: i64,
    bookmark_id: i64,
) -> Result<(), String> {
    let deleted = conn
        .execute(
            "DELETE FROM reader_bookmarks WHERE id = ?1 AND archive_id = ?2",
            params![bookmark_id, archive_id],
        )
        .map_err(|e| format!("删除书签失败: {e}"))?;

    if deleted == 0 {
        return Err("书签不存在".to_string());
    }

    Ok(())
}

pub fn add_tag(conn: &Connection, archive_id: i64, name: &str) -> Result<Tag, String> {
    let name = normalize_label(name, 40, "标签")?;
    ensure_archive_exists(conn, archive_id)?;

    let existing: Option<(i64, Option<i64>, Option<i64>)> = conn
        .query_row(
            "SELECT id, deleted_at, merged_into
             FROM tags WHERE name = ?1 COLLATE NOCASE LIMIT 1",
            params![name],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|e| format!("查询标签失败: {e}"))?;

    let tag_id = match existing {
        Some((id, deleted_at, merged_into)) => {
            let merge_target = merged_into.and_then(|target| {
                conn.query_row(
                    "SELECT id FROM tags WHERE id = ?1 AND deleted_at IS NULL",
                    params![target],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
                .ok()
                .flatten()
            });

            if let Some(target) = merge_target {
                target
            } else {
                if deleted_at.is_some() {
                    conn.execute(
                        "UPDATE tags SET deleted_at = NULL, merged_into = NULL WHERE id = ?1",
                        params![id],
                    )
                    .map_err(|e| format!("恢复回收站标签失败: {e}"))?;
                }
                id
            }
        }
        None => {
            let translated = translate::translate_tag(&name).map(str::to_string);
            let source = translated.as_ref().map(|_| "dictionary");
            let translated_at = translated.as_ref().map(|_| fsutil::now_ms());
            conn.execute(
                "INSERT INTO tags(
                    name, original_name, translated_name, translation_source, translated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![name, name, translated, source, translated_at],
            )
            .map_err(|e| format!("创建标签失败: {e}"))?;
            conn.last_insert_rowid()
        }
    };

    conn.execute(
        "INSERT OR IGNORE INTO archive_tags(archive_id, tag_id) VALUES (?1, ?2)",
        params![archive_id, tag_id],
    )
    .map_err(|e| format!("为画集添加标签失败: {e}"))?;

    tag_by_id(conn, tag_id)
}

pub fn remove_tag(conn: &Connection, archive_id: i64, tag_id: i64) -> Result<(), String> {
    conn.execute(
        "DELETE FROM archive_tags WHERE archive_id = ?1 AND tag_id = ?2",
        params![archive_id, tag_id],
    )
    .map_err(|e| format!("移除标签失败: {e}"))?;

    Ok(())
}

pub fn update_tag(
    conn: &Connection,
    tag_id: i64,
    name: &str,
    translated_name: Option<&str>,
) -> Result<Tag, String> {
    let name = normalize_label(name, 40, "标签")?;
    let translated_name = translated_name
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| normalize_label(value, 40, "中文译名"))
        .transpose()?;

    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM tags WHERE name = ?1 COLLATE NOCASE AND id <> ?2 LIMIT 1",
            params![name, tag_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("检查标签失败: {e}"))?;

    if existing.is_some() {
        return Err("已存在同名标签（可能在回收站）".to_string());
    }

    let source = translated_name.as_ref().map(|_| "manual");
    let translated_at = translated_name.as_ref().map(|_| fsutil::now_ms());
    let changed = conn
        .execute(
            "UPDATE tags SET
                name = ?1,
                translated_name = ?2,
                translation_source = ?3,
                translated_at = ?4
             WHERE id = ?5 AND deleted_at IS NULL",
            params![name, translated_name, source, translated_at, tag_id],
        )
        .map_err(|e| format!("更新标签失败: {e}"))?;

    if changed == 0 {
        return Err("标签不存在或已在回收站".to_string());
    }

    tag_by_id(conn, tag_id)
}

const MAX_CATEGORY_DEPTH: usize = 6;
const MAX_CATEGORY_SEGMENT: usize = 40;

fn normalize_category_path(raw: &str) -> Result<String, String> {
    let mut segments = Vec::new();
    for segment in raw.split('/') {
        let trimmed = segment.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.chars().count() > MAX_CATEGORY_SEGMENT {
            return Err(format!("每级分类名称最多 {MAX_CATEGORY_SEGMENT} 个字符"));
        }
        segments.push(trimmed.to_string());
    }
    if segments.is_empty() {
        return Err("分类不能为空".to_string());
    }
    if segments.len() > MAX_CATEGORY_DEPTH {
        return Err(format!("分类最多 {MAX_CATEGORY_DEPTH} 级"));
    }
    Ok(segments.join("/"))
}

fn normalize_category_name(raw: &str) -> Result<String, String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err("分类名称不能为空".to_string());
    }
    if name.contains('/') {
        return Err("分类名称不能包含斜杠，请先选择上级分类".to_string());
    }
    if name.chars().count() > MAX_CATEGORY_SEGMENT {
        return Err(format!("分类名称最多 {MAX_CATEGORY_SEGMENT} 个字符"));
    }
    Ok(name.to_string())
}

fn ensure_category_path(conn: &Connection, raw: &str) -> Result<String, String> {
    let normalized = normalize_category_path(raw)?;
    let mut parent_id: Option<i64> = None;
    let mut canonical = String::new();

    for segment in normalized.split('/') {
        if !canonical.is_empty() {
            canonical.push('/');
        }
        canonical.push_str(segment);

        let existing: Option<(i64, String)> = conn
            .query_row(
                "SELECT id, path FROM tag_categories WHERE path = ?1 COLLATE NOCASE",
                params![canonical],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|e| format!("查询分类失败: {e}"))?;

        match existing {
            Some((id, path)) => {
                parent_id = Some(id);
                canonical = path;
            }
            None => {
                conn.execute(
                    "INSERT INTO tag_categories(name, parent_id, path, created_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![segment, parent_id, canonical, fsutil::now_ms()],
                )
                .map_err(|e| format!("创建分类失败: {e}"))?;
                parent_id = Some(conn.last_insert_rowid());
            }
        }
    }

    Ok(canonical)
}

fn read_tag_category(row: &rusqlite::Row<'_>) -> rusqlite::Result<TagCategory> {
    let path: String = row.get(3)?;
    Ok(TagCategory {
        id: row.get(0)?,
        name: row.get(1)?,
        parent_id: row.get(2)?,
        depth: path.matches('/').count() as i64,
        path,
    })
}

fn category_by_path(conn: &Connection, path: &str) -> Result<TagCategory, String> {
    conn.query_row(
        "SELECT id, name, parent_id, path FROM tag_categories
         WHERE path = ?1 COLLATE NOCASE",
        params![path],
        read_tag_category,
    )
    .optional()
    .map_err(|e| format!("查询分类失败: {e}"))?
    .ok_or_else(|| "分类不存在，可能已被删除".to_string())
}

pub fn list_tag_categories(conn: &Connection) -> Result<Vec<TagCategory>, String> {
    let mut statement = conn
        .prepare(
            "SELECT id, name, parent_id, path FROM tag_categories
             ORDER BY path COLLATE NOCASE",
        )
        .map_err(|e| format!("准备分类查询失败: {e}"))?;
    let rows = statement
        .query_map([], read_tag_category)
        .map_err(|e| format!("查询分类失败: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取分类失败: {e}"))
}

pub fn create_tag_category(
    conn: &Connection,
    parent_path: Option<&str>,
    name: &str,
) -> Result<TagCategory, String> {
    let name = normalize_category_name(name)?;
    let parent = match parent_path.map(str::trim).filter(|value| !value.is_empty()) {
        Some(path) => {
            let normalized = normalize_category_path(path)?;
            Some(category_by_path(conn, &normalized)?)
        }
        None => None,
    };

    if parent.as_ref().map(|category| category.depth as usize + 1) >= Some(MAX_CATEGORY_DEPTH) {
        return Err(format!("分类最多 {MAX_CATEGORY_DEPTH} 级"));
    }

    let path = match &parent {
        Some(category) => format!("{}/{}", category.path, name),
        None => name.clone(),
    };

    let exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tag_categories WHERE path = ?1 COLLATE NOCASE",
            params![path],
            |row| row.get(0),
        )
        .map_err(|e| format!("检查分类失败: {e}"))?;
    if exists > 0 {
        return Err(format!("分类「{path}」已存在"));
    }

    conn.execute(
        "INSERT INTO tag_categories(name, parent_id, path, created_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![name, parent.as_ref().map(|category| category.id), path, fsutil::now_ms()],
    )
    .map_err(|e| format!("创建分类失败: {e}"))?;

    category_by_path(conn, &path)
}

fn rewrite_category_paths(
    conn: &Connection,
    old_path: &str,
    new_path: &str,
    category_id: i64,
) -> Result<(), String> {
    let old_prefix = format!("{old_path}/");
    let new_prefix = format!("{new_path}/");
    let prefix_len = old_prefix.chars().count() as i64;

    conn.execute(
        "UPDATE tag_categories
         SET path = ?1 || substr(path, ?2 + 1)
         WHERE substr(path, 1, ?2) = ?3 AND id <> ?4",
        params![new_prefix, prefix_len, old_prefix, category_id],
    )
    .map_err(|e| format!("更新子分类路径失败: {e}"))?;

    conn.execute(
        "UPDATE tags
         SET category = ?1 || substr(category, ?2 + 1)
         WHERE substr(category, 1, ?2) = ?3",
        params![new_prefix, prefix_len, old_prefix],
    )
    .map_err(|e| format!("更新标签分类路径失败: {e}"))?;

    conn.execute(
        "UPDATE tags SET category = ?1 WHERE category = ?2",
        params![new_path, old_path],
    )
    .map_err(|e| format!("更新标签分类失败: {e}"))?;

    Ok(())
}

pub fn rename_tag_category(
    conn: &mut Connection,
    path: &str,
    new_name: &str,
) -> Result<Vec<TagCategory>, String> {
    let category = category_by_path(conn, path)?;
    let name = normalize_category_name(new_name)?;
    let parent_path = category
        .path
        .rsplit_once('/')
        .map(|(parent, _)| parent.to_string());
    let new_path = match &parent_path {
        Some(parent) => format!("{parent}/{name}"),
        None => name.clone(),
    };

    if new_path != category.path {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM tag_categories WHERE path = ?1 COLLATE NOCASE",
                params![new_path],
                |row| row.get(0),
            )
            .map_err(|e| format!("检查分类失败: {e}"))?;
        if exists > 0 {
            return Err(format!("分类「{new_path}」已存在"));
        }
    }

    let tx = conn
        .transaction()
        .map_err(|e| format!("开启分类事务失败: {e}"))?;
    tx.execute(
        "UPDATE tag_categories SET name = ?1, path = ?2 WHERE id = ?3",
        params![name, new_path, category.id],
    )
    .map_err(|e| format!("重命名分类失败: {e}"))?;
    rewrite_category_paths(&tx, &category.path, &new_path, category.id)?;
    tx.commit()
        .map_err(|e| format!("保存分类失败: {e}"))?;

    list_tag_categories(conn)
}

pub fn move_tag_category(
    conn: &mut Connection,
    path: &str,
    new_parent_path: Option<&str>,
) -> Result<Vec<TagCategory>, String> {
    let category = category_by_path(conn, path)?;
    let parent = match new_parent_path
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(parent) => {
            let normalized = normalize_category_path(parent)?;
            Some(category_by_path(conn, &normalized)?)
        }
        None => None,
    };

    if let Some(parent) = &parent {
        if parent.path == category.path
            || parent.path.starts_with(&format!("{}/", category.path))
        {
            return Err("不能把分类移动到它自身或其子分类下".to_string());
        }
        let height = category_subtree_height(conn, &category.path)?;
        if parent.depth as usize + 1 + height >= MAX_CATEGORY_DEPTH {
            return Err(format!("移动后分类层级超过 {MAX_CATEGORY_DEPTH} 级"));
        }
    }

    let new_path = match &parent {
        Some(parent) => format!("{}/{}", parent.path, category.name),
        None => category.name.clone(),
    };
    if new_path == category.path {
        return list_tag_categories(conn);
    }

    let exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tag_categories WHERE path = ?1 COLLATE NOCASE",
            params![new_path],
            |row| row.get(0),
        )
        .map_err(|e| format!("检查分类失败: {e}"))?;
    if exists > 0 {
        return Err(format!("分类「{new_path}」已存在"));
    }

    let tx = conn
        .transaction()
        .map_err(|e| format!("开启分类事务失败: {e}"))?;
    tx.execute(
        "UPDATE tag_categories SET parent_id = ?1, path = ?2 WHERE id = ?3",
        params![parent.as_ref().map(|item| item.id), new_path, category.id],
    )
    .map_err(|e| format!("移动分类失败: {e}"))?;
    rewrite_category_paths(&tx, &category.path, &new_path, category.id)?;
    tx.commit()
        .map_err(|e| format!("保存分类失败: {e}"))?;

    list_tag_categories(conn)
}

fn category_subtree_height(conn: &Connection, path: &str) -> Result<usize, String> {
    let own_depth = path.matches('/').count();
    let prefix = format!("{path}/");
    let prefix_len = prefix.chars().count() as i64;
    let mut statement = conn
        .prepare(
            "SELECT path FROM tag_categories
             WHERE substr(path, 1, ?1) = ?2",
        )
        .map_err(|e| format!("准备子分类查询失败: {e}"))?;
    let rows = statement
        .query_map(params![prefix_len, prefix], |row| row.get::<_, String>(0))
        .map_err(|e| format!("查询子分类失败: {e}"))?;
    let mut height = 0usize;
    for row in rows {
        let row = row.map_err(|e| format!("读取子分类失败: {e}"))?;
        height = height.max(row.matches('/').count().saturating_sub(own_depth));
    }
    Ok(height)
}

pub fn delete_tag_category(
    conn: &mut Connection,
    path: &str,
) -> Result<Vec<TagCategory>, String> {
    let category = category_by_path(conn, path)?;
    let old_prefix = format!("{}/", category.path);
    let prefix_len = old_prefix.chars().count() as i64;

    let tx = conn
        .transaction()
        .map_err(|e| format!("开启分类事务失败: {e}"))?;
    tx.execute(
        "UPDATE tags SET category = NULL
         WHERE category = ?1 OR substr(category, 1, ?2) = ?3",
        params![category.path, prefix_len, old_prefix],
    )
    .map_err(|e| format!("清除标签分类失败: {e}"))?;
    tx.execute(
        "DELETE FROM tag_categories WHERE id = ?1",
        params![category.id],
    )
    .map_err(|e| format!("删除分类失败: {e}"))?;
    tx.commit()
        .map_err(|e| format!("保存分类失败: {e}"))?;

    list_tag_categories(conn)
}

fn backfill_tag_categories(conn: &Connection) -> Result<(), String> {
    let mut statement = conn
        .prepare(
            "SELECT DISTINCT category FROM tags
             WHERE category IS NOT NULL AND TRIM(category) <> ''",
        )
        .map_err(|e| format!("准备分类迁移失败: {e}"))?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| format!("查询旧分类失败: {e}"))?;
    let legacy = rows
        .collect::<Result<Vec<String>, _>>()
        .map_err(|e| format!("读取旧分类失败: {e}"))?;

    for raw in legacy {
        let mut segments = Vec::new();
        for segment in raw.split('/') {
            let trimmed = segment.trim();
            if trimmed.is_empty() {
                continue;
            }
            let limited = trimmed.chars().take(MAX_CATEGORY_SEGMENT).collect::<String>();
            segments.push(limited);
        }
        if segments.is_empty() || segments.len() > MAX_CATEGORY_DEPTH {
            continue;
        }
        let path = segments.join("/");
        if let Ok(canonical) = ensure_category_path(conn, &path) {
            if canonical != raw {
                conn.execute(
                    "UPDATE tags SET category = ?1 WHERE category = ?2",
                    params![canonical, raw],
                )
                .map_err(|e| format!("迁移标签分类失败: {e}"))?;
            }
        }
    }

    Ok(())
}

pub fn set_tag_category(
    conn: &Connection,
    tag_id: i64,
    category: Option<&str>,
) -> Result<Tag, String> {
    let category = match category.map(str::trim).filter(|value| !value.is_empty()) {
        Some(path) => Some(ensure_category_path(conn, path)?),
        None => None,
    };
    let changed = conn
        .execute(
            "UPDATE tags SET category = ?1 WHERE id = ?2 AND deleted_at IS NULL",
            params![category, tag_id],
        )
        .map_err(|e| format!("更新标签分类失败: {e}"))?;
    if changed == 0 {
        return Err("标签不存在或已在回收站".to_string());
    }
    tag_by_id(conn, tag_id)
}

pub fn set_tag_translation(
    conn: &Connection,
    tag_id: i64,
    translated_name: Option<&str>,
    source: &str,
) -> Result<Tag, String> {
    let translated_name = translated_name
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| normalize_label(value, 40, "中文译名"))
        .transpose()?;
    let source = translated_name.as_ref().map(|_| source);
    let translated_at = translated_name.as_ref().map(|_| fsutil::now_ms());

    let changed = conn
        .execute(
            "UPDATE tags SET
                translated_name = ?1,
                translation_source = ?2,
                translated_at = ?3
             WHERE id = ?4 AND deleted_at IS NULL",
            params![translated_name, source, translated_at, tag_id],
        )
        .map_err(|e| format!("写入标签翻译失败: {e}"))?;

    if changed == 0 {
        return Err("标签不存在或已在回收站".to_string());
    }

    tag_by_id(conn, tag_id)
}

pub fn set_tag_translations_batch(
    conn: &mut Connection,
    updates: &[TagTranslationInput],
) -> Result<usize, String> {
    let tx = conn
        .transaction()
        .map_err(|e| format!("开启翻译事务失败: {e}"))?;
    let mut applied = 0usize;

    for update in updates {
        let source = update.source.as_deref().unwrap_or("ai");
        let translated_name = update
            .translated_name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| normalize_label(value, 40, "中文译名"))
            .transpose()?;
        let source = translated_name.as_ref().map(|_| source);
        let translated_at = translated_name.as_ref().map(|_| fsutil::now_ms());
        let changed = tx
            .execute(
                "UPDATE tags SET
                    translated_name = ?1,
                    translation_source = ?2,
                    translated_at = ?3
                 WHERE id = ?4 AND deleted_at IS NULL",
                params![translated_name, source, translated_at, update.id],
            )
            .map_err(|e| format!("批量写入标签翻译失败: {e}"))?;
        if changed == 0 {
            return Err(format!(
                "标签 #{} 不存在或已在回收站，批量写入已回滚",
                update.id
            ));
        }
        applied += 1;
    }

    tx.commit()
        .map_err(|e| format!("提交翻译事务失败: {e}"))?;
    Ok(applied)
}

pub fn merge_tags(
    conn: &mut Connection,
    source_ids: &[i64],
    target_id: i64,
) -> Result<Tag, String> {
    let target_deleted: Option<Option<i64>> = conn
        .query_row(
            "SELECT deleted_at FROM tags WHERE id = ?1",
            params![target_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("检查目标标签失败: {e}"))?;

    match target_deleted {
        Some(None) => {}
        Some(Some(_)) => return Err("目标标签在回收站，不能作为合并目标".to_string()),
        None => return Err("目标标签不存在".to_string()),
    }

    let mut sources: Vec<i64> = Vec::new();
    for id in source_ids {
        if *id != target_id && *id > 0 && !sources.contains(id) {
            sources.push(*id);
        }
    }
    if sources.is_empty() {
        return Err("请选择至少一个与目标不同的来源标签".to_string());
    }

    let tx = conn
        .transaction()
        .map_err(|e| format!("开启合并事务失败: {e}"))?;

    for source_id in &sources {
        let deleted: Option<Option<i64>> = tx
            .query_row(
                "SELECT deleted_at FROM tags WHERE id = ?1",
                params![source_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("检查来源标签失败: {e}"))?;
        match deleted {
            Some(None) => {}
            Some(Some(_)) => {
                return Err(format!("来源标签 #{source_id} 已在回收站，已回滚"));
            }
            None => return Err(format!("来源标签 #{source_id} 不存在，已回滚")),
        }

        tx.execute(
            "INSERT OR IGNORE INTO archive_tags(archive_id, tag_id)
             SELECT archive_id, ?1 FROM archive_tags WHERE tag_id = ?2",
            params![target_id, source_id],
        )
        .map_err(|e| format!("迁移标签关联失败: {e}"))?;
        tx.execute(
            "DELETE FROM archive_tags WHERE tag_id = ?1",
            params![source_id],
        )
        .map_err(|e| format!("清理来源标签关联失败: {e}"))?;
        tx.execute(
            "UPDATE tags SET deleted_at = ?1, merged_into = ?2 WHERE id = ?3",
            params![fsutil::now_ms(), target_id, source_id],
        )
        .map_err(|e| format!("标记来源标签失败: {e}"))?;
    }

    tx.commit()
        .map_err(|e| format!("提交合并事务失败: {e}"))?;
    tag_by_id(conn, target_id)
}

pub fn delete_tag(conn: &Connection, tag_id: i64) -> Result<(), String> {
    let changed = conn
        .execute(
            "UPDATE tags SET deleted_at = ?1
             WHERE id = ?2 AND deleted_at IS NULL",
            params![fsutil::now_ms(), tag_id],
        )
        .map_err(|e| format!("删除标签失败: {e}"))?;

    if changed == 0 {
        return Err("标签不存在或已在回收站".to_string());
    }

    Ok(())
}

pub fn restore_tag(conn: &Connection, tag_id: i64) -> Result<Tag, String> {
    let merged_into: Option<Option<i64>> = conn
        .query_row(
            "SELECT merged_into FROM tags WHERE id = ?1 AND deleted_at IS NOT NULL",
            params![tag_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("检查回收站标签失败: {e}"))?;

    match merged_into {
        Some(Some(target)) => {
            return Err(format!(
                "该标签已合并到 #{target}，不能直接恢复；可恢复目标标签"
            ));
        }
        Some(None) => {}
        None => return Err("回收站里没有这个标签".to_string()),
    }

    conn.execute(
        "UPDATE tags SET deleted_at = NULL WHERE id = ?1",
        params![tag_id],
    )
    .map_err(|e| format!("恢复标签失败: {e}"))?;

    tag_by_id(conn, tag_id)
}

pub fn purge_tag(conn: &Connection, tag_id: i64) -> Result<(), String> {
    let changed = conn
        .execute(
            "DELETE FROM tags WHERE id = ?1 AND deleted_at IS NOT NULL",
            params![tag_id],
        )
        .map_err(|e| format!("彻底删除标签失败: {e}"))?;

    if changed == 0 {
        return Err("只能彻底删除回收站中的标签".to_string());
    }

    Ok(())
}

pub fn tag_graph(conn: &Connection, min_weight: i64, limit: usize) -> Result<TagGraph, String> {
    let nodes = list_tags(conn)?;

    let mut statement = conn
        .prepare(
            "SELECT at1.tag_id, at2.tag_id, COUNT(*) AS weight
             FROM archive_tags at1
             JOIN archive_tags at2
               ON at1.archive_id = at2.archive_id
              AND at1.tag_id < at2.tag_id
             JOIN archives ar ON ar.id = at1.archive_id AND ar.deleted_at IS NULL
             JOIN tags ta ON ta.id = at1.tag_id AND ta.deleted_at IS NULL
             JOIN tags tb ON tb.id = at2.tag_id AND tb.deleted_at IS NULL
             GROUP BY at1.tag_id, at2.tag_id
             HAVING COUNT(*) >= ?1
             ORDER BY weight DESC, at1.tag_id, at2.tag_id
             LIMIT ?2",
        )
        .map_err(|e| format!("准备导图查询失败: {e}"))?;

    let edges = statement
        .query_map(params![min_weight.max(1), limit.clamp(1, 2000) as i64], |row| {
            Ok(TagEdge {
                source: row.get(0)?,
                target: row.get(1)?,
                weight: row.get(2)?,
            })
        })
        .map_err(|e| format!("查询标签关联失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取标签关联失败: {e}"))?;

    Ok(TagGraph { nodes, edges })
}

const COLLECTION_SELECT: &str = "c.id, c.name, c.is_default, COUNT(caa.id),
     c.sort_order, c.cover_archive_id, a.cover_key, c.deleted_at";

fn read_collection(
    row: &rusqlite::Row<'_>,
    covers_dir: Option<&Path>,
) -> rusqlite::Result<Collection> {
    let is_default: i64 = row.get(2)?;
    let cover_key: Option<String> = row.get(6)?;
    let cover_path = match (covers_dir, cover_key) {
        (Some(dir), Some(key)) => Some(dir.join(key).to_string_lossy().to_string()),
        _ => None,
    };
    Ok(Collection {
        id: row.get(0)?,
        name: row.get(1)?,
        is_default: is_default != 0,
        count: row.get(3)?,
        sort_order: row.get(4)?,
        cover_archive_id: row.get(5)?,
        cover_path,
        deleted_at: row.get(7)?,
    })
}

pub fn list_collections(
    conn: &Connection,
    covers_dir: Option<&Path>,
) -> Result<Vec<Collection>, String> {
    let mut statement = conn
        .prepare(&format!(
            "SELECT {COLLECTION_SELECT}
             FROM collections c
             LEFT JOIN collection_archives ca ON ca.collection_id = c.id
             LEFT JOIN archives caa ON caa.id = ca.archive_id AND caa.deleted_at IS NULL
             LEFT JOIN archives a ON a.id = c.cover_archive_id
             WHERE c.deleted_at IS NULL
             GROUP BY c.id
             ORDER BY c.is_default DESC, c.sort_order ASC, c.created_at ASC, c.id ASC"
        ))
        .map_err(|e| format!("准备收藏夹查询失败: {e}"))?;

    let rows = statement
        .query_map([], |row| read_collection(row, covers_dir))
        .map_err(|e| format!("查询收藏夹失败: {e}"))?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取收藏夹失败: {e}"))
}

pub fn list_deleted_collections(
    conn: &Connection,
    covers_dir: Option<&Path>,
) -> Result<Vec<Collection>, String> {
    let mut statement = conn
        .prepare(&format!(
            "SELECT {COLLECTION_SELECT}
             FROM collections c
             LEFT JOIN collection_archives ca ON ca.collection_id = c.id
             LEFT JOIN archives caa ON caa.id = ca.archive_id AND caa.deleted_at IS NULL
             LEFT JOIN archives a ON a.id = c.cover_archive_id
             WHERE c.deleted_at IS NOT NULL
             GROUP BY c.id
             ORDER BY c.deleted_at DESC, c.id DESC"
        ))
        .map_err(|e| format!("准备收藏夹回收站查询失败: {e}"))?;

    let rows = statement
        .query_map([], |row| read_collection(row, covers_dir))
        .map_err(|e| format!("查询收藏夹回收站失败: {e}"))?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取收藏夹回收站失败: {e}"))
}

fn collection_by_id(
    conn: &Connection,
    collection_id: i64,
    covers_dir: Option<&Path>,
) -> Result<Collection, String> {
    conn.query_row(
        &format!(
            "SELECT {COLLECTION_SELECT}
             FROM collections c
             LEFT JOIN collection_archives ca ON ca.collection_id = c.id
             LEFT JOIN archives caa ON caa.id = ca.archive_id AND caa.deleted_at IS NULL
             LEFT JOIN archives a ON a.id = c.cover_archive_id
             WHERE c.id = ?1 AND c.deleted_at IS NULL
             GROUP BY c.id"
        ),
        params![collection_id],
        |row| read_collection(row, covers_dir),
    )
    .optional()
    .map_err(|e| format!("读取收藏夹失败: {e}"))?
    .ok_or_else(|| "收藏夹不存在".to_string())
}

pub fn create_collection(conn: &Connection, name: &str) -> Result<Collection, String> {
    let name = normalize_label(name, 24, "收藏夹名称")?;

    let existing: Option<(i64, Option<i64>)> = conn
        .query_row(
            "SELECT id, deleted_at FROM collections WHERE name = ?1 COLLATE NOCASE LIMIT 1",
            params![name],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|e| format!("检查收藏夹失败: {e}"))?;

    if let Some((_, deleted_at)) = existing {
        return Err(if deleted_at.is_some() {
            "回收站中已有同名收藏夹，请先恢复或彻底删除".to_string()
        } else {
            "已存在同名收藏夹".to_string()
        });
    }

    let sort_order: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(sort_order), 0) + 1 FROM collections",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("计算收藏夹顺序失败: {e}"))?;

    conn.execute(
        "INSERT INTO collections(name, is_default, created_at, sort_order)
         VALUES (?1, 0, ?2, ?3)",
        params![name, fsutil::now_ms(), sort_order],
    )
    .map_err(|e| format!("创建收藏夹失败: {e}"))?;

    collection_by_id(conn, conn.last_insert_rowid(), None)
}

pub fn rename_collection(
    conn: &Connection,
    collection_id: i64,
    name: &str,
) -> Result<Collection, String> {
    let name = normalize_label(name, 24, "收藏夹名称")?;
    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM collections WHERE name = ?1 COLLATE NOCASE AND id <> ?2 LIMIT 1",
            params![name, collection_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("检查收藏夹失败: {e}"))?;

    if existing.is_some() {
        return Err("已存在同名收藏夹".to_string());
    }

    let changed = conn
        .execute(
            "UPDATE collections SET name = ?1
             WHERE id = ?2 AND deleted_at IS NULL",
            params![name, collection_id],
        )
        .map_err(|e| format!("重命名收藏夹失败: {e}"))?;

    if changed == 0 {
        return Err("收藏夹不存在或已在回收站".to_string());
    }

    collection_by_id(conn, collection_id, None)
}

pub fn delete_collection(conn: &Connection, collection_id: i64) -> Result<(), String> {
    let found: Option<(i64, Option<i64>)> = conn
        .query_row(
            "SELECT is_default, deleted_at FROM collections WHERE id = ?1",
            params![collection_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|e| format!("检查收藏夹失败: {e}"))?;

    match found {
        Some((1, _)) => return Err("默认收藏夹不能删除".to_string()),
        Some((_, Some(_))) => return Err("收藏夹已在回收站".to_string()),
        Some(_) => {}
        None => return Err("收藏夹不存在".to_string()),
    }

    conn.execute(
        "UPDATE collections SET deleted_at = ?1 WHERE id = ?2",
        params![fsutil::now_ms(), collection_id],
    )
    .map_err(|e| format!("移入回收站失败: {e}"))?;
    Ok(())
}

pub fn restore_collection(
    conn: &Connection,
    collection_id: i64,
    covers_dir: Option<&Path>,
) -> Result<Collection, String> {
    let deleted_at: Option<Option<i64>> = conn
        .query_row(
            "SELECT deleted_at FROM collections WHERE id = ?1",
            params![collection_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("检查收藏夹失败: {e}"))?;

    match deleted_at {
        None => return Err("收藏夹不存在".to_string()),
        Some(None) => return Err("收藏夹不在回收站".to_string()),
        Some(Some(_)) => {}
    }

    conn.execute(
        "UPDATE collections SET deleted_at = NULL WHERE id = ?1",
        params![collection_id],
    )
    .map_err(|e| format!("恢复收藏夹失败: {e}"))?;

    collection_by_id(conn, collection_id, covers_dir)
}

pub fn purge_collection(conn: &Connection, collection_id: i64) -> Result<(), String> {
    let deleted_at: Option<Option<i64>> = conn
        .query_row(
            "SELECT deleted_at FROM collections WHERE id = ?1",
            params![collection_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("检查收藏夹失败: {e}"))?;

    match deleted_at {
        None => return Err("收藏夹不存在".to_string()),
        Some(None) => return Err("只能彻底删除回收站中的收藏夹".to_string()),
        Some(Some(_)) => {}
    }

    conn.execute(
        "DELETE FROM collections WHERE id = ?1",
        params![collection_id],
    )
    .map_err(|e| format!("彻底删除收藏夹失败: {e}"))?;
    Ok(())
}

pub fn reorder_collections(conn: &mut Connection, ordered_ids: &[i64]) -> Result<(), String> {
    let tx = conn
        .transaction()
        .map_err(|e| format!("开启排序事务失败: {e}"))?;

    for (index, id) in ordered_ids.iter().enumerate() {
        tx.execute(
            "UPDATE collections SET sort_order = ?1
             WHERE id = ?2 AND deleted_at IS NULL",
            params![index as i64 + 1, id],
        )
        .map_err(|e| format!("更新收藏夹顺序失败: {e}"))?;
    }

    tx.commit()
        .map_err(|e| format!("提交收藏夹排序失败: {e}"))
}

pub fn set_collection_cover(
    conn: &Connection,
    collection_id: i64,
    archive_id: Option<i64>,
) -> Result<Collection, String> {
    collection_by_id(conn, collection_id, None)?;

    if let Some(archive_id) = archive_id {
        let member: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM collection_archives
                 WHERE collection_id = ?1 AND archive_id = ?2",
                params![collection_id, archive_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("检查收藏夹成员失败: {e}"))?;
        if member.is_none() {
            return Err("只能把收藏夹内的画集设为封面".to_string());
        }
    }

    conn.execute(
        "UPDATE collections SET cover_archive_id = ?1 WHERE id = ?2",
        params![archive_id, collection_id],
    )
    .map_err(|e| format!("设置收藏夹封面失败: {e}"))?;

    collection_by_id(conn, collection_id, None)
}

pub fn add_archives_to_collection(
    conn: &mut Connection,
    collection_id: i64,
    archive_ids: &[i64],
) -> Result<usize, String> {
    collection_by_id(conn, collection_id, None)?;
    let tx = conn
        .transaction()
        .map_err(|e| format!("开启收藏事务失败: {e}"))?;
    let now = fsutil::now_ms();
    let mut added = 0usize;

    for archive_id in archive_ids {
        let changed = tx
            .execute(
                "INSERT OR IGNORE INTO collection_archives(collection_id, archive_id, added_at)
                 VALUES (?1, ?2, ?3)",
                params![collection_id, archive_id, now],
            )
            .map_err(|e| format!("批量加入收藏夹失败: {e}"))?;
        added += changed;
    }

    tx.commit()
        .map_err(|e| format!("提交收藏事务失败: {e}"))?;
    Ok(added)
}

pub fn archive_collections(conn: &Connection, archive_id: i64) -> Result<Vec<i64>, String> {
    let mut statement = conn
        .prepare(
            "SELECT ca.collection_id
             FROM collection_archives ca
             JOIN collections c ON c.id = ca.collection_id AND c.deleted_at IS NULL
             WHERE ca.archive_id = ?1",
        )
        .map_err(|e| format!("准备收藏夹查询失败: {e}"))?;

    let rows = statement
        .query_map(params![archive_id], |row| row.get(0))
        .map_err(|e| format!("查询画集收藏夹失败: {e}"))?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取画集收藏夹失败: {e}"))
}

pub fn set_archive_collection(
    conn: &Connection,
    archive_id: i64,
    collection_id: i64,
    member: bool,
) -> Result<(), String> {
    ensure_archive_exists(conn, archive_id)?;

    let collection_exists: Option<i64> = conn
        .query_row(
            "SELECT id FROM collections WHERE id = ?1 AND deleted_at IS NULL",
            params![collection_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("检查收藏夹失败: {e}"))?;

    if collection_exists.is_none() {
        return Err("收藏夹不存在或已在回收站".to_string());
    }

    if member {
        add_to_collection(conn, collection_id, archive_id)
    } else {
        conn.execute(
            "DELETE FROM collection_archives
             WHERE collection_id = ?1 AND archive_id = ?2",
            params![collection_id, archive_id],
        )
        .map_err(|e| format!("移出收藏夹失败: {e}"))?;
        Ok(())
    }
}
