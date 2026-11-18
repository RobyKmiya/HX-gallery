use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use archive_gallery_lib::db::{self, LibraryContext};
use archive_gallery_lib::fsutil;
use archive_gallery_lib::importer;
use archive_gallery_lib::library;
use archive_gallery_lib::models::{
    ArchiveQuery, ImportOptions, QueueFillOptions, TagTranslationInput,
};
use archive_gallery_lib::queue;
use clap::{Parser, Subcommand, ValueEnum};
use serde_json::{json, Value};

#[derive(Debug, Parser)]
#[command(
    name = "galleryctl",
    version,
    about = "Local JSON interface for AI agents managing an archive-gallery library"
)]
struct Cli {
    #[arg(long, global = true, env = "ARCHIVE_GALLERY_LIBRARY")]
    library: Option<PathBuf>,
    #[arg(long, global = true)]
    compact: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Tags {
        #[command(subcommand)]
        command: TagCommand,
    },
    Collections {
        #[command(subcommand)]
        command: CollectionCommand,
    },
    /// 待看目录：按标签权重随机挑画集排队。
    Queue {
        #[command(subcommand)]
        command: QueueCommand,
    },
    Search {
        #[arg(long)]
        query: Option<String>,
        #[arg(long, default_value_t = 1)]
        page: u32,
        #[arg(long, default_value_t = 48)]
        limit: u32,
        #[arg(long)]
        sort: Option<String>,
        #[arg(long)]
        direction: Option<String>,
        #[arg(long)]
        collection_id: Option<i64>,
        /// 只看某个待看目录里的画集。
        #[arg(long)]
        queue_id: Option<i64>,
        #[arg(long, value_delimiter = ',')]
        tag_ids: Vec<i64>,
        #[arg(long, default_value = "and")]
        tag_mode: String,
        #[arg(long, value_delimiter = ',')]
        rating_buckets: Vec<i64>,
        #[arg(long = "read-status", value_delimiter = ',')]
        read_statuses: Vec<String>,
        #[arg(long)]
        missing: bool,
    },
    Archives {
        #[command(subcommand)]
        command: ArchiveCommand,
    },
    Init,
    Scan {
        #[arg(long)]
        covers_dir: Option<PathBuf>,
    },
    Import {
        #[arg(long, required = true)]
        paths: Vec<PathBuf>,
        #[arg(long)]
        covers_dir: Option<PathBuf>,
        #[arg(long)]
        collection_id: Option<i64>,
    },
    Stats,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum TagStatus {
    Active,
    Untranslated,
    Translated,
    Unused,
    Deleted,
}

#[derive(Debug, Subcommand)]
enum TagCommand {
    List {
        #[arg(long, value_enum, default_value_t = TagStatus::Active)]
        status: TagStatus,
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long)]
        after: Option<i64>,
        #[arg(long)]
        query: Option<String>,
    },
    Translate {
        #[arg(long)]
        input: String,
    },
    Update {
        #[arg(long)]
        id: i64,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        translated_name: Option<String>,
        #[arg(long)]
        clear_translation: bool,
        #[arg(long)]
        category: Option<String>,
    },
    Merge {
        #[arg(long, value_delimiter = ',')]
        source: Vec<i64>,
        #[arg(long)]
        target: i64,
    },
    Delete {
        #[arg(long, value_delimiter = ',')]
        ids: Vec<i64>,
        #[arg(long)]
        purge: bool,
    },
    Restore {
        #[arg(long, value_delimiter = ',')]
        ids: Vec<i64>,
    },
    Attach {
        #[arg(long)]
        archive: i64,
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
    },
    Detach {
        #[arg(long)]
        archive: i64,
        #[arg(long, value_delimiter = ',')]
        tag_ids: Vec<i64>,
    },
    Graph {
        #[arg(long, default_value_t = 1)]
        min_weight: i64,
        #[arg(long, default_value_t = 300)]
        limit: usize,
    },
    Stats,
}

#[derive(Debug, Subcommand)]
enum CollectionCommand {
    List,
    Create {
        #[arg(long)]
        name: String,
    },
    Rename {
        #[arg(long)]
        id: i64,
        #[arg(long)]
        name: String,
    },
    Delete {
        #[arg(long)]
        id: i64,
    },
    Trash,
    Restore {
        #[arg(long)]
        id: i64,
    },
    Purge {
        #[arg(long)]
        id: i64,
    },
    Reorder {
        #[arg(long, value_delimiter = ',')]
        ids: Vec<i64>,
    },
    Cover {
        #[arg(long)]
        id: i64,
        #[arg(long)]
        archive_id: Option<i64>,
        #[arg(long)]
        clear: bool,
    },
    Add {
        #[arg(long)]
        id: i64,
        #[arg(long, value_delimiter = ',')]
        archives: Vec<i64>,
    },
    Remove {
        #[arg(long)]
        id: i64,
        #[arg(long, value_delimiter = ',')]
        archives: Vec<i64>,
    },
}

#[derive(Debug, Subcommand)]
enum ArchiveCommand {
    Note {
        #[arg(long)]
        id: i64,
        #[arg(long)]
        text: Option<String>,
        #[arg(long)]
        clear: bool,
    },
}

#[derive(Debug, Subcommand)]
enum QueueCommand {
    /// 列出全部待看目录（含进度）。
    List,
    /// 生成（或续满）一个待看目录。
    Fill {
        /// 目录名；不传则用今天日期。
        #[arg(long)]
        name: Option<String>,
        #[arg(long, default_value_t = 10)]
        size: i64,
        /// 只在这些标签里挑（逗号分隔 id）。
        #[arg(long, value_delimiter = ',')]
        tag_ids: Vec<i64>,
        /// 允许挑已经看过的。
        #[arg(long)]
        include_read: bool,
    },
    Delete {
        #[arg(long)]
        id: i64,
    },
    /// 列出某个目录里的画集 id。
    Archives {
        #[arg(long)]
        id: i64,
    },
}

fn main() {
    let cli = Cli::parse();
    let compact = cli.compact;
    match run(cli) {
        Ok(value) => {
            let text = if compact {
                serde_json::to_string(&value).unwrap()
            } else {
                serde_json::to_string_pretty(&value).unwrap()
            };
            println!("{text}");
        }
        Err(error) => {
            let payload = json!({ "ok": false, "error": error });
            eprintln!("{}", serde_json::to_string(&payload).unwrap());
            std::process::exit(1);
        }
    }
}

fn run(cli: Cli) -> Result<Value, String> {
    let path = resolve_library(&cli)?;

    if matches!(&cli.command, Command::Init) {
        let library = db::open_library(&path)?;
        return Ok(json!({
            "ok": true,
            "data": {
                "library": library.root.to_string_lossy(),
                "name": library.library_name,
                "initialized": true
            }
        }));
    }

    let mut library = open_existing(&path)?;

    let data = match cli.command {
        Command::Tags { command } => run_tags(command, &mut library)?,
        Command::Collections { command } => run_collections(command, &mut library)?,
        Command::Queue { command } => run_queue(command, &mut library)?,
        Command::Search {
            query,
            page,
            limit,
            sort,
            direction,
            collection_id,
            queue_id,
            tag_ids,
            tag_mode,
            rating_buckets,
            read_statuses,
            missing,
        } => {
            // 原来这里传的是 Path::new("")，返回给调用方的 coverPath 只有裸文件名，
            // 没有目录前缀，等于不可用。与 scan / import 一样解析真实缓存目录。
            let covers_dir = default_covers_dir(&library)?;
            let page = library::query_archives(
                &library.conn,
                &library.root,
                &covers_dir,
                &ArchiveQuery {
                    page: Some(page),
                    page_size: Some(limit),
                    sort,
                    direction,
                    collection_id,
                    queue_id,
                    tag_ids: if tag_ids.is_empty() {
                        None
                    } else {
                        Some(tag_ids)
                    },
                    tag_mode: Some(tag_mode),
                    rating_buckets: if rating_buckets.is_empty() {
                        None
                    } else {
                        Some(rating_buckets)
                    },
                    read_statuses: if read_statuses.is_empty() {
                        None
                    } else {
                        Some(read_statuses)
                    },
                    missing_only: if missing { Some(true) } else { None },
                    search: query,
                    group: None,
                },
            )?;
            serde_json::to_value(page).map_err(|e| format!("序列化查询结果失败: {e}"))?
        }
        Command::Archives { command } => run_archives(command, &library)?,
        Command::Init => unreachable!("init handled before opening an existing library"),
        Command::Scan { covers_dir } => {
            let covers_dir = match covers_dir {
                Some(dir) => dir,
                None => default_covers_dir(&library)?,
            };
            fs::create_dir_all(&covers_dir)
                .map_err(|e| format!("无法创建封面缓存目录: {e}"))?;
            eprintln!("scanning {} ...", library.root.to_string_lossy());
            let report = library::scan_connection(&library.conn, &library.root, &covers_dir);
            serde_json::to_value(report).map_err(|e| format!("序列化扫描结果失败: {e}"))?
        }
        Command::Import {
            paths,
            covers_dir,
            collection_id,
        } => {
            let covers_dir = match covers_dir {
                Some(dir) => dir,
                None => default_covers_dir(&library)?,
            };
            fs::create_dir_all(&covers_dir)
                .map_err(|e| format!("无法创建封面缓存目录: {e}"))?;
            let report = importer::import_on_connection(
                &library.conn,
                &library.root,
                &covers_dir,
                paths,
                ImportOptions {
                    add_to_collection: collection_id,
                    preserve_structure: None,
                    tags: Vec::new(),
                },
                |_| {},
            );
            serde_json::to_value(report).map_err(|e| format!("序列化导入结果失败: {e}"))?
        }
        Command::Stats => library_stats(&library)?,
    };

    Ok(json!({ "ok": true, "data": data }))
}

fn run_tags(command: TagCommand, library: &mut LibraryContext) -> Result<Value, String> {
    match command {
        TagCommand::List {
            status,
            limit,
            after,
            query,
        } => {
            let mut tags = match status {
                TagStatus::Deleted => db::list_deleted_tags(&library.conn)?,
                _ => db::list_tags(&library.conn)?,
            };

            if matches!(status, TagStatus::Untranslated) {
                tags.retain(|tag| tag.translated_name.as_deref().unwrap_or("").is_empty());
            }
            if matches!(status, TagStatus::Translated) {
                tags.retain(|tag| tag.translated_name.as_deref().unwrap_or("").is_empty() == false);
            }
            if matches!(status, TagStatus::Unused) {
                tags.retain(|tag| tag.count == 0);
            }
            if let Some(after) = after {
                tags.retain(|tag| tag.id > after);
            }
            if let Some(query) = query.as_deref().map(str::trim).filter(|value| !value.is_empty()) {
                let needle = query.to_lowercase();
                tags.retain(|tag| {
                    tag.name.to_lowercase().contains(&needle)
                        || tag.original_name.to_lowercase().contains(&needle)
                        || tag
                            .translated_name
                            .as_deref()
                            .map(|value| value.to_lowercase().contains(&needle))
                            .unwrap_or(false)
                });
            }

            tags.sort_by_key(|tag| tag.id);
            let matched = tags.len();
            if let Some(limit) = limit {
                tags.truncate(limit);
            }
            let next_after = tags.last().map(|tag| tag.id);
            Ok(json!({
                "status": format!("{status:?}"),
                "matched": matched,
                "returned": tags.len(),
                "nextAfter": next_after,
                "tags": tags
            }))
        }
        TagCommand::Translate { input } => {
            let raw = read_input(&input)?;
            let root: Value = serde_json::from_str(&raw)
                .map_err(|e| format!("翻译输入不是合法 JSON: {e}"))?;
            let items = if let Some(array) = root.as_array() {
                array
            } else {
                root.get("translations")
                    .and_then(Value::as_array)
                    .ok_or_else(|| {
                        "翻译输入应为数组，或包含 translations 数组的对象".to_string()
                    })?
            };

            let mut updates = Vec::with_capacity(items.len());
            for item in items {
                let id = json_i64(item.get("id").or_else(|| item.get("tagId"))).ok_or_else(|| {
                    "每条翻译都需要 id（数字）".to_string()
                })?;
                let translated_name = item
                    .get("translatedName")
                    .or_else(|| item.get("translated_name"))
                    .and_then(Value::as_str)
                    .map(|value| value.to_string());
                let source = item
                    .get("source")
                    .and_then(Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or("ai")
                    .to_string();
                updates.push(TagTranslationInput {
                    id,
                    translated_name,
                    source: Some(source),
                });
            }

            let applied = db::set_tag_translations_batch(&mut library.conn, &updates)?;
            Ok(json!({ "applied": applied, "ids": updates.iter().map(|item| item.id).collect::<Vec<_>>() }))
        }
        TagCommand::Update {
            id,
            name,
            translated_name,
            clear_translation,
            category,
        } => {
            let current = db::list_tags(&library.conn)?
                .into_iter()
                .find(|tag| tag.id == id)
                .ok_or_else(|| format!("标签 #{id} 不存在或已在回收站"))?;
            let next_name = name.unwrap_or(current.name);
            let next_translation = if clear_translation {
                None
            } else {
                translated_name.or(current.translated_name)
            };
            let mut tag = db::update_tag(
                &library.conn,
                id,
                &next_name,
                next_translation.as_deref(),
            )?;
            if let Some(category) = category {
                tag = db::set_tag_category(&library.conn, id, Some(&category))?;
            }
            Ok(serde_json::to_value(tag).map_err(|e| format!("序列化标签失败: {e}"))?)
        }
        TagCommand::Merge { source, target } => {
            let tag = db::merge_tags(&mut library.conn, &source, target)?;
            Ok(serde_json::to_value(tag).map_err(|e| format!("序列化标签失败: {e}"))?)
        }
        TagCommand::Delete { ids, purge } => {
            let mut deleted = Vec::new();
            let mut failed = Vec::new();
            for id in ids {
                let result = if purge {
                    db::purge_tag(&library.conn, id)
                } else {
                    db::delete_tag(&library.conn, id)
                };
                match result {
                    Ok(()) => deleted.push(id),
                    Err(error) => failed.push(json!({ "id": id, "error": error })),
                }
            }
            Ok(json!({ "deleted": deleted, "failed": failed, "purge": purge }))
        }
        TagCommand::Restore { ids } => {
            let mut restored = Vec::new();
            let mut failed = Vec::new();
            for id in ids {
                match db::restore_tag(&library.conn, id) {
                    Ok(tag) => restored.push(tag),
                    Err(error) => failed.push(json!({ "id": id, "error": error })),
                }
            }
            Ok(json!({ "restored": restored, "failed": failed }))
        }
        TagCommand::Attach { archive, tags } => {
            let mut attached = Vec::new();
            let mut failed = Vec::new();
            for name in tags {
                match db::add_tag(&library.conn, archive, &name) {
                    Ok(tag) => attached.push(tag),
                    Err(error) => {
                        failed.push(json!({ "name": name, "error": error }))
                    }
                }
            }
            Ok(json!({
                "archive": archive,
                "attached": attached,
                "failed": failed
            }))
        }
        TagCommand::Detach { archive, tag_ids } => {
            let mut detached = Vec::new();
            let mut failed = Vec::new();
            for id in tag_ids {
                match db::remove_tag(&library.conn, archive, id) {
                    Ok(()) => detached.push(id),
                    Err(error) => failed.push(json!({ "id": id, "error": error })),
                }
            }
            Ok(json!({
                "archive": archive,
                "detached": detached,
                "failed": failed
            }))
        }
        TagCommand::Graph { min_weight, limit } => {
            let graph = db::tag_graph(&library.conn, min_weight, limit)?;
            Ok(serde_json::to_value(graph).map_err(|e| format!("序列化导图失败: {e}"))?)
        }
        TagCommand::Stats => {
            let active = db::list_tags(&library.conn)?;
            let deleted = db::list_deleted_tags(&library.conn)?;
            let translated = active
                .iter()
                .filter(|tag| tag.translated_name.as_deref().unwrap_or("").is_empty() == false)
                .count();
            let used = active.iter().filter(|tag| tag.count > 0).count();
            Ok(json!({
                "total": active.len(),
                "used": used,
                "unused": active.len() - used,
                "translated": translated,
                "untranslated": active.len() - translated,
                "deleted": deleted.len(),
                "merged": deleted.iter().filter(|tag| tag.merged_into.is_some()).count()
            }))
        }
    }
}

fn run_queue(command: QueueCommand, library: &mut db::LibraryContext) -> Result<Value, String> {
    // 外层 run() 已统一包成 {ok, data}，这里只返回 data 本体。
    match command {
        QueueCommand::List => {
            let queues = queue::list_queues(&library.conn)?;
            Ok(json!({ "queues": queues }))
        }
        QueueCommand::Fill {
            name,
            size,
            tag_ids,
            include_read,
        } => {
            let name = name.unwrap_or_else(today_name);
            let options = QueueFillOptions {
                size: Some(size),
                tag_ids,
                include_read: Some(include_read),
            };
            let report = queue::fill_queue(&mut library.conn, &name, &options, None)?;
            serde_json::to_value(report).map_err(|e| format!("序列化待看目录失败: {e}"))
        }
        QueueCommand::Delete { id } => {
            queue::delete_queue(&library.conn, id)?;
            Ok(json!({ "deleted": id }))
        }
        QueueCommand::Archives { id } => {
            let ids = queue::queue_archives(&library.conn, id)?;
            Ok(json!({ "archives": ids }))
        }
    }
}

/// 今天的日期，用作待看目录的默认名字（本地时区，YYYY-MM-DD）。
fn today_name() -> String {
    // chrono 不在依赖里，这里按 UTC+8 粗略换算：本地库的使用者都在东八区。
    let now = fsutil::now_ms() / 1000 + 8 * 3600;
    let days = now.div_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

/// 由「1970-01-01 起的天数」反推年月日（Howard Hinnant 的算法）。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn run_collections(
    command: CollectionCommand,
    library: &mut LibraryContext,
) -> Result<Value, String> {
    match command {
        CollectionCommand::List => {
            let collections = db::list_collections(&library.conn, None)?;
            Ok(serde_json::to_value(collections)
                .map_err(|e| format!("序列化收藏夹失败: {e}"))?)
        }
        CollectionCommand::Create { name } => {
            let collection = db::create_collection(&library.conn, &name)?;
            Ok(serde_json::to_value(collection)
                .map_err(|e| format!("序列化收藏夹失败: {e}"))?)
        }
        CollectionCommand::Rename { id, name } => {
            let collection = db::rename_collection(&library.conn, id, &name)?;
            Ok(serde_json::to_value(collection)
                .map_err(|e| format!("序列化收藏夹失败: {e}"))?)
        }
        CollectionCommand::Delete { id } => {
            db::delete_collection(&library.conn, id)?;
            Ok(json!({ "trashed": id }))
        }
        CollectionCommand::Trash => {
            let collections = db::list_deleted_collections(&library.conn, None)?;
            Ok(serde_json::to_value(collections)
                .map_err(|e| format!("序列化收藏夹回收站失败: {e}"))?)
        }
        CollectionCommand::Restore { id } => {
            let collection = db::restore_collection(&library.conn, id, None)?;
            Ok(serde_json::to_value(collection)
                .map_err(|e| format!("序列化收藏夹失败: {e}"))?)
        }
        CollectionCommand::Purge { id } => {
            db::purge_collection(&library.conn, id)?;
            Ok(json!({ "purged": id }))
        }
        CollectionCommand::Reorder { ids } => {
            db::reorder_collections(&mut library.conn, &ids)?;
            Ok(json!({ "ordered": ids }))
        }
        CollectionCommand::Cover {
            id,
            archive_id,
            clear,
        } => {
            let archive_id = if clear { None } else { archive_id };
            let collection = db::set_collection_cover(&library.conn, id, archive_id)?;
            Ok(serde_json::to_value(collection)
                .map_err(|e| format!("序列化收藏夹失败: {e}"))?)
        }
        CollectionCommand::Add { id, archives } => {
            let added = db::add_archives_to_collection(&mut library.conn, id, &archives)?;
            Ok(json!({ "collectionId": id, "added": added }))
        }
        CollectionCommand::Remove { id, archives } => {
            let mut removed = Vec::new();
            let mut failed = Vec::new();
            for archive_id in archives {
                match db::set_archive_collection(&library.conn, archive_id, id, false) {
                    Ok(()) => removed.push(archive_id),
                    Err(error) => failed.push(json!({ "archiveId": archive_id, "error": error })),
                }
            }
            Ok(json!({ "collectionId": id, "removed": removed, "failed": failed }))
        }
    }
}

fn run_archives(command: ArchiveCommand, library: &LibraryContext) -> Result<Value, String> {
    match command {
        ArchiveCommand::Note { id, text, clear } => {
            let note = if clear { None } else { text };
            let saved = db::set_note(&library.conn, id, note.as_deref())?;
            Ok(json!({ "id": id, "note": saved }))
        }
    }
}

fn library_stats(library: &LibraryContext) -> Result<Value, String> {
    let (archives, total_size, missing) = db::stats(&library.conn)?;
    let tags = db::list_tags(&library.conn)?;
    let collections = db::list_collections(&library.conn, None)?;
    Ok(json!({
        "library": library.root.to_string_lossy(),
        "name": library.library_name,
        "archives": archives,
        "totalSize": total_size,
        "missing": missing,
        "tags": tags.len(),
        "collections": collections.len()
    }))
}

fn resolve_library(cli: &Cli) -> Result<PathBuf, String> {
    if let Some(path) = &cli.library {
        return Ok(path.clone());
    }

    if let Some(path) = std::env::var_os("ARCHIVE_GALLERY_LIBRARY") {
        return Ok(PathBuf::from(path));
    }

    let config_root = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config"))
            })
    }
    .ok_or_else(|| "无法定位应用配置目录，请用 --library 指定库目录".to_string())?;

    let settings_path = config_root
        .join("com.archivegallery.desktop")
        .join("settings.json");
    let raw = fs::read_to_string(&settings_path).map_err(|e| {
        format!(
            "无法读取应用设置 {}: {e}；请用 --library 指定库目录",
            settings_path.to_string_lossy()
        )
    })?;
    let settings: Value =
        serde_json::from_str(&raw).map_err(|e| format!("应用设置格式错误: {e}"))?;
    settings
        .get("library_path")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .ok_or_else(|| "应用设置里没有 library_path，请用 --library 指定库目录".to_string())
}

fn default_covers_dir(library: &LibraryContext) -> Result<PathBuf, String> {
    let cache_root = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
    }
    .ok_or_else(|| "无法定位缓存目录，请用 --covers-dir 指定".to_string())?;

    Ok(cache_root
        .join("com.archivegallery.desktop")
        .join("covers")
        .join(&library.library_id))
}

fn open_existing(path: &Path) -> Result<LibraryContext, String> {
    if !path.join(".gallery").join("library.db").exists() {
        return Err(format!(
            "{} 还不是已初始化的画廊库；请先在应用中打开该目录，或确认路径正确",
            path.to_string_lossy()
        ));
    }
    db::open_library(path)
}

fn read_input(input: &str) -> Result<String, String> {
    if input == "-" {
        let mut raw = String::new();
        std::io::stdin()
            .read_to_string(&mut raw)
            .map_err(|e| format!("读取标准输入失败: {e}"))?;
        Ok(raw)
    } else {
        fs::read_to_string(input).map_err(|e| format!("读取 {input} 失败: {e}"))
    }
}

fn json_i64(value: Option<&Value>) -> Option<i64> {
    value.and_then(|value| {
        value
            .as_i64()
            .or_else(|| value.as_str().and_then(|raw| raw.parse().ok()))
    })
}
