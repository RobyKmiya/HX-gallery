mod archive;
pub mod db;
pub mod fsutil;
pub mod importer;
pub mod library;
pub mod models;
mod paths;
mod reader;
pub mod translate;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

use models::{
    ArchivePage, ArchiveQuery, Collection, ImportOptions, ImportReport, LibraryState,
    ReaderBookmark, ReaderInfo, ReaderPage, ScanReport, Tag, TagCategory, TagGraph,
    TagTranslationInput, TrashedArchive,
};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct AppSettings {
    library_path: Option<String>,
}

struct AppInner {
    library: Option<db::LibraryContext>,
    last_library_path: Option<String>,
    startup_error: Option<String>,
}

pub struct AppState {
    inner: Mutex<AppInner>,
}

impl AppState {
    fn new(app: &AppHandle) -> Self {
        let settings = read_settings(app);
        let mut inner = AppInner {
            library: None,
            last_library_path: settings.library_path.clone(),
            startup_error: None,
        };

        if let Some(path) = settings.library_path {
            let library_path = PathBuf::from(&path);
            let initialized = library_path.is_dir()
                && library_path.join(".gallery").join("library.db").exists();

            if initialized {
                match db::open_library(&library_path) {
                    Ok(library) => {
                        eprintln!(
                            "opened library at {}",
                            library.root.to_string_lossy()
                        );
                        inner.library = Some(library);
                    }
                    Err(error) => {
                        eprintln!("failed to open library: {error}");
                        inner.startup_error = Some(format!("上次的库无法打开：{error}"));
                    }
                }
            } else {
                inner.startup_error = Some("上次的库目录不存在或尚未初始化".to_string());
            }
        }

        Self {
            inner: Mutex::new(inner),
        }
    }
}

fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|error| format!("无法获取配置目录: {error}"))?;
    fs::create_dir_all(&config_dir).map_err(|error| format!("无法创建配置目录: {error}"))?;
    Ok(config_dir.join("settings.json"))
}

fn read_settings(app: &AppHandle) -> AppSettings {
    let Ok(path) = settings_path(app) else {
        return AppSettings::default();
    };
    let Ok(raw) = fs::read_to_string(path) else {
        return AppSettings::default();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

fn write_settings(app: &AppHandle, settings: &AppSettings) {
    let Ok(path) = settings_path(app) else {
        eprintln!("failed to resolve settings path");
        return;
    };
    match serde_json::to_string_pretty(settings) {
        Ok(raw) => {
            if let Err(error) = fs::write(path, raw) {
                eprintln!("failed to write settings: {error}");
            }
        }
        Err(error) => eprintln!("failed to serialize settings: {error}"),
    }
}

fn build_library_state(state: &AppState) -> Result<LibraryState, String> {
    let inner = state
        .inner
        .lock()
        .map_err(|_| "应用状态已损坏".to_string())?;

    if let Some(library) = &inner.library {
        let (archive_count, total_size, missing_count) = db::stats(&library.conn)?;
        Ok(LibraryState {
            library_path: Some(library.root.to_string_lossy().to_string()),
            library_name: Some(library.library_name.clone()),
            archive_count,
            missing_count,
            total_size,
            last_library_path: inner.last_library_path.clone(),
            error: None,
        })
    } else {
        Ok(LibraryState {
            library_path: None,
            library_name: None,
            archive_count: 0,
            missing_count: 0,
            total_size: 0,
            last_library_path: inner.last_library_path.clone(),
            error: inner.startup_error.clone(),
        })
    }
}

#[tauri::command]
fn get_library_state(state: State<'_, AppState>) -> Result<LibraryState, String> {
    build_library_state(&state)
}

#[tauri::command]
fn open_library(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> Result<LibraryState, String> {
    if path.trim().is_empty() {
        return Err("库目录不能为空".to_string());
    }

    let library = db::open_library(Path::new(&path))?;
    let display_path = library.root.to_string_lossy().to_string();

    {
        let mut inner = state
            .inner
            .lock()
            .map_err(|_| "应用状态已损坏".to_string())?;
        inner.library = Some(library);
        inner.last_library_path = Some(display_path.clone());
        inner.startup_error = None;
    }

    write_settings(
        &app,
        &AppSettings {
            library_path: Some(display_path),
        },
    );

    build_library_state(&state)
}

#[tauri::command]
async fn import_paths(
    app: AppHandle,
    state: State<'_, AppState>,
    paths: Vec<String>,
    options: ImportOptions,
) -> Result<ImportReport, String> {
    let (root, covers_dir) = {
        let inner = state
            .inner
            .lock()
            .map_err(|_| "应用状态已损坏".to_string())?;
        let library = inner
            .library
            .as_ref()
            .ok_or_else(|| "请先选择库目录".to_string())?;
        let covers_dir = paths::covers_dir(&app, &library.library_id)?;
        (library.root.clone(), covers_dir)
    };

    let input_paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();

    tauri::async_runtime::spawn_blocking(move || {
        importer::import_blocking(
            &app,
            &root,
            &covers_dir,
            input_paths,
            options,
        )
    })
    .await
    .map_err(|error| format!("导入任务异常结束: {error}"))
}

#[tauri::command]
async fn scan_library(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<ScanReport, String> {
    let (root, library_id) = {
        let inner = state
            .inner
            .lock()
            .map_err(|_| "应用状态已损坏".to_string())?;
        let library = inner
            .library
            .as_ref()
            .ok_or_else(|| "请先选择库目录".to_string())?;
        (library.root.clone(), library.library_id.clone())
    };

    tauri::async_runtime::spawn_blocking(move || {
        library::scan_blocking(&app, &root, &library_id)
    })
    .await
    .map_err(|error| format!("扫描任务异常结束: {error}"))
}

#[tauri::command]
fn set_rating(state: State<'_, AppState>, id: i64, rating: i64) -> Result<i64, String> {
    let inner = state
        .inner
        .lock()
        .map_err(|_| "应用状态已损坏".to_string())?;
    let library = inner
        .library
        .as_ref()
        .ok_or_else(|| "请先选择库目录".to_string())?;

    db::set_rating(&library.conn, id, rating)?;
    Ok(rating)
}

#[tauri::command]
fn list_tags(state: State<'_, AppState>) -> Result<Vec<Tag>, String> {
    let inner = state
        .inner
        .lock()
        .map_err(|_| "应用状态已损坏".to_string())?;
    let library = inner
        .library
        .as_ref()
        .ok_or_else(|| "请先选择库目录".to_string())?;
    db::list_tags(&library.conn)
}

#[tauri::command]
fn list_tag_categories(state: State<'_, AppState>) -> Result<Vec<TagCategory>, String> {
    let inner = state
        .inner
        .lock()
        .map_err(|_| "应用状态已损坏".to_string())?;
    let library = inner
        .library
        .as_ref()
        .ok_or_else(|| "请先选择库目录".to_string())?;
    db::list_tag_categories(&library.conn)
}

#[tauri::command]
fn create_tag_category(
    state: State<'_, AppState>,
    parent_path: Option<String>,
    name: String,
) -> Result<TagCategory, String> {
    let inner = state
        .inner
        .lock()
        .map_err(|_| "应用状态已损坏".to_string())?;
    let library = inner
        .library
        .as_ref()
        .ok_or_else(|| "请先选择库目录".to_string())?;
    db::create_tag_category(&library.conn, parent_path.as_deref(), &name)
}

#[tauri::command]
fn rename_tag_category(
    state: State<'_, AppState>,
    path: String,
    new_name: String,
) -> Result<Vec<TagCategory>, String> {
    let mut inner = state
        .inner
        .lock()
        .map_err(|_| "应用状态已损坏".to_string())?;
    let library = inner
        .library
        .as_mut()
        .ok_or_else(|| "请先选择库目录".to_string())?;
    db::rename_tag_category(&mut library.conn, &path, &new_name)
}

#[tauri::command]
fn move_tag_category(
    state: State<'_, AppState>,
    path: String,
    new_parent_path: Option<String>,
) -> Result<Vec<TagCategory>, String> {
    let mut inner = state
        .inner
        .lock()
        .map_err(|_| "应用状态已损坏".to_string())?;
    let library = inner
        .library
        .as_mut()
        .ok_or_else(|| "请先选择库目录".to_string())?;
    db::move_tag_category(&mut library.conn, &path, new_parent_path.as_deref())
}

#[tauri::command]
fn delete_tag_category(
    state: State<'_, AppState>,
    path: String,
) -> Result<Vec<TagCategory>, String> {
    let mut inner = state
        .inner
        .lock()
        .map_err(|_| "应用状态已损坏".to_string())?;
    let library = inner
        .library
        .as_mut()
        .ok_or_else(|| "请先选择库目录".to_string())?;
    db::delete_tag_category(&mut library.conn, &path)
}

#[tauri::command]
fn set_tag_category(
    state: State<'_, AppState>,
    tag_id: i64,
    category: Option<String>,
) -> Result<Tag, String> {
    let inner = state
        .inner
        .lock()
        .map_err(|_| "应用状态已损坏".to_string())?;
    let library = inner
        .library
        .as_ref()
        .ok_or_else(|| "请先选择库目录".to_string())?;
    db::set_tag_category(&library.conn, tag_id, category.as_deref())
}

#[tauri::command]
fn list_archive_tags(
    state: State<'_, AppState>,
    archive_id: i64,
) -> Result<Vec<Tag>, String> {
    let inner = state
        .inner
        .lock()
        .map_err(|_| "应用状态已损坏".to_string())?;
    let library = inner
        .library
        .as_ref()
        .ok_or_else(|| "请先选择库目录".to_string())?;
    db::archive_tags(&library.conn, archive_id)
}

#[tauri::command]
fn add_tag(
    state: State<'_, AppState>,
    archive_id: i64,
    name: String,
) -> Result<Tag, String> {
    let inner = state
        .inner
        .lock()
        .map_err(|_| "应用状态已损坏".to_string())?;
    let library = inner
        .library
        .as_ref()
        .ok_or_else(|| "请先选择库目录".to_string())?;
    db::add_tag(&library.conn, archive_id, &name)
}

#[tauri::command]
fn remove_tag(
    state: State<'_, AppState>,
    archive_id: i64,
    tag_id: i64,
) -> Result<(), String> {
    let inner = state
        .inner
        .lock()
        .map_err(|_| "应用状态已损坏".to_string())?;
    let library = inner
        .library
        .as_ref()
        .ok_or_else(|| "请先选择库目录".to_string())?;
    db::remove_tag(&library.conn, archive_id, tag_id)
}

#[tauri::command]
fn update_tag(
    state: State<'_, AppState>,
    tag_id: i64,
    name: String,
    translated_name: Option<String>,
) -> Result<Tag, String> {
    let inner = state
        .inner
        .lock()
        .map_err(|_| "应用状态已损坏".to_string())?;
    let library = inner
        .library
        .as_ref()
        .ok_or_else(|| "请先选择库目录".to_string())?;
    db::update_tag(
        &library.conn,
        tag_id,
        &name,
        translated_name.as_deref(),
    )
}

#[tauri::command]
fn delete_tag(state: State<'_, AppState>, tag_id: i64) -> Result<(), String> {
    let inner = state
        .inner
        .lock()
        .map_err(|_| "应用状态已损坏".to_string())?;
    let library = inner
        .library
        .as_ref()
        .ok_or_else(|| "请先选择库目录".to_string())?;
    db::delete_tag(&library.conn, tag_id)
}

#[tauri::command]
fn tag_graph(
    state: State<'_, AppState>,
    min_weight: Option<i64>,
    limit: Option<usize>,
) -> Result<TagGraph, String> {
    let inner = state
        .inner
        .lock()
        .map_err(|_| "应用状态已损坏".to_string())?;
    let library = inner
        .library
        .as_ref()
        .ok_or_else(|| "请先选择库目录".to_string())?;
    db::tag_graph(
        &library.conn,
        min_weight.unwrap_or(1),
        limit.unwrap_or(300),
    )
}

#[tauri::command]
fn delete_archive(state: State<'_, AppState>, archive_id: i64) -> Result<(), String> {
    let inner = state
        .inner
        .lock()
        .map_err(|_| "应用状态已损坏".to_string())?;
    let library = inner
        .library
        .as_ref()
        .ok_or_else(|| "请先选择库目录".to_string())?;
    db::delete_archive(&library.conn, archive_id)
}

#[tauri::command]
fn restore_archive(state: State<'_, AppState>, archive_id: i64) -> Result<(), String> {
    let inner = state
        .inner
        .lock()
        .map_err(|_| "应用状态已损坏".to_string())?;
    let library = inner
        .library
        .as_ref()
        .ok_or_else(|| "请先选择库目录".to_string())?;
    db::restore_archive(&library.conn, archive_id)
}

#[tauri::command]
fn purge_archive(
    app: AppHandle,
    state: State<'_, AppState>,
    archive_id: i64,
) -> Result<(), String> {
    let inner = state
        .inner
        .lock()
        .map_err(|_| "应用状态已损坏".to_string())?;
    let library = inner
        .library
        .as_ref()
        .ok_or_else(|| "请先选择库目录".to_string())?;
    let covers_dir = paths::covers_dir(&app, &library.library_id)?;
    let cover_key = db::purge_archive(&library.conn, archive_id)?;
    if let Some(key) = cover_key {
        let _ = fs::remove_file(covers_dir.join(key));
    }
    Ok(())
}

#[tauri::command]
fn list_deleted_archives(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<TrashedArchive>, String> {
    let inner = state
        .inner
        .lock()
        .map_err(|_| "应用状态已损坏".to_string())?;
    let library = inner
        .library
        .as_ref()
        .ok_or_else(|| "请先选择库目录".to_string())?;
    let covers_dir = paths::covers_dir(&app, &library.library_id)?;
    db::list_deleted_archives(&library.conn, Some(&covers_dir))
}

#[tauri::command]
fn list_deleted_tags(state: State<'_, AppState>) -> Result<Vec<Tag>, String> {
    let inner = state
        .inner
        .lock()
        .map_err(|_| "应用状态已损坏".to_string())?;
    let library = inner
        .library
        .as_ref()
        .ok_or_else(|| "请先选择库目录".to_string())?;
    db::list_deleted_tags(&library.conn)
}

#[tauri::command]
fn restore_tag(state: State<'_, AppState>, tag_id: i64) -> Result<Tag, String> {
    let inner = state
        .inner
        .lock()
        .map_err(|_| "应用状态已损坏".to_string())?;
    let library = inner
        .library
        .as_ref()
        .ok_or_else(|| "请先选择库目录".to_string())?;
    db::restore_tag(&library.conn, tag_id)
}

#[tauri::command]
fn purge_tag(state: State<'_, AppState>, tag_id: i64) -> Result<(), String> {
    let inner = state
        .inner
        .lock()
        .map_err(|_| "应用状态已损坏".to_string())?;
    let library = inner
        .library
        .as_ref()
        .ok_or_else(|| "请先选择库目录".to_string())?;
    db::purge_tag(&library.conn, tag_id)
}

#[tauri::command]
fn merge_tags(
    state: State<'_, AppState>,
    source_ids: Vec<i64>,
    target_id: i64,
) -> Result<Tag, String> {
    let mut inner = state
        .inner
        .lock()
        .map_err(|_| "应用状态已损坏".to_string())?;
    let library = inner
        .library
        .as_mut()
        .ok_or_else(|| "请先选择库目录".to_string())?;
    db::merge_tags(&mut library.conn, &source_ids, target_id)
}

#[tauri::command]
fn set_tag_translations(
    state: State<'_, AppState>,
    updates: Vec<TagTranslationInput>,
) -> Result<usize, String> {
    let mut inner = state
        .inner
        .lock()
        .map_err(|_| "应用状态已损坏".to_string())?;
    let library = inner
        .library
        .as_mut()
        .ok_or_else(|| "请先选择库目录".to_string())?;
    db::set_tag_translations_batch(&mut library.conn, &updates)
}

#[tauri::command]
fn set_note(
    state: State<'_, AppState>,
    id: i64,
    note: Option<String>,
) -> Result<Option<String>, String> {
    let inner = state
        .inner
        .lock()
        .map_err(|_| "应用状态已损坏".to_string())?;
    let library = inner
        .library
        .as_ref()
        .ok_or_else(|| "请先选择库目录".to_string())?;
    db::set_note(&library.conn, id, note.as_deref())
}

#[tauri::command]
fn list_collections(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<Collection>, String> {
    let inner = state
        .inner
        .lock()
        .map_err(|_| "应用状态已损坏".to_string())?;
    let library = inner
        .library
        .as_ref()
        .ok_or_else(|| "请先选择库目录".to_string())?;
    let covers_dir = paths::covers_dir(&app, &library.library_id)?;
    db::list_collections(&library.conn, Some(&covers_dir))
}

#[tauri::command]
fn create_collection(
    state: State<'_, AppState>,
    name: String,
) -> Result<Collection, String> {
    let inner = state
        .inner
        .lock()
        .map_err(|_| "应用状态已损坏".to_string())?;
    let library = inner
        .library
        .as_ref()
        .ok_or_else(|| "请先选择库目录".to_string())?;
    db::create_collection(&library.conn, &name)
}

#[tauri::command]
fn list_archive_collections(
    state: State<'_, AppState>,
    archive_id: i64,
) -> Result<Vec<i64>, String> {
    let inner = state
        .inner
        .lock()
        .map_err(|_| "应用状态已损坏".to_string())?;
    let library = inner
        .library
        .as_ref()
        .ok_or_else(|| "请先选择库目录".to_string())?;
    db::archive_collections(&library.conn, archive_id)
}

#[tauri::command]
fn set_archive_collection(
    state: State<'_, AppState>,
    archive_id: i64,
    collection_id: i64,
    member: bool,
) -> Result<(), String> {
    let inner = state
        .inner
        .lock()
        .map_err(|_| "应用状态已损坏".to_string())?;
    let library = inner
        .library
        .as_ref()
        .ok_or_else(|| "请先选择库目录".to_string())?;
    db::set_archive_collection(&library.conn, archive_id, collection_id, member)
}

#[tauri::command]
fn rename_collection(
    state: State<'_, AppState>,
    collection_id: i64,
    name: String,
) -> Result<Collection, String> {
    let inner = state
        .inner
        .lock()
        .map_err(|_| "应用状态已损坏".to_string())?;
    let library = inner
        .library
        .as_ref()
        .ok_or_else(|| "请先选择库目录".to_string())?;
    db::rename_collection(&library.conn, collection_id, &name)
}

#[tauri::command]
fn delete_collection(state: State<'_, AppState>, collection_id: i64) -> Result<(), String> {
    let inner = state
        .inner
        .lock()
        .map_err(|_| "应用状态已损坏".to_string())?;
    let library = inner
        .library
        .as_ref()
        .ok_or_else(|| "请先选择库目录".to_string())?;
    db::delete_collection(&library.conn, collection_id)
}

#[tauri::command]
fn list_deleted_collections(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<Collection>, String> {
    let inner = state
        .inner
        .lock()
        .map_err(|_| "应用状态已损坏".to_string())?;
    let library = inner
        .library
        .as_ref()
        .ok_or_else(|| "请先选择库目录".to_string())?;
    let covers_dir = paths::covers_dir(&app, &library.library_id)?;
    db::list_deleted_collections(&library.conn, Some(&covers_dir))
}

#[tauri::command]
fn restore_collection(
    app: AppHandle,
    state: State<'_, AppState>,
    collection_id: i64,
) -> Result<Collection, String> {
    let inner = state
        .inner
        .lock()
        .map_err(|_| "应用状态已损坏".to_string())?;
    let library = inner
        .library
        .as_ref()
        .ok_or_else(|| "请先选择库目录".to_string())?;
    let covers_dir = paths::covers_dir(&app, &library.library_id)?;
    db::restore_collection(&library.conn, collection_id, Some(&covers_dir))
}

#[tauri::command]
fn purge_collection(state: State<'_, AppState>, collection_id: i64) -> Result<(), String> {
    let inner = state
        .inner
        .lock()
        .map_err(|_| "应用状态已损坏".to_string())?;
    let library = inner
        .library
        .as_ref()
        .ok_or_else(|| "请先选择库目录".to_string())?;
    db::purge_collection(&library.conn, collection_id)
}

#[tauri::command]
fn reorder_collections(
    state: State<'_, AppState>,
    ordered_ids: Vec<i64>,
) -> Result<(), String> {
    let mut inner = state
        .inner
        .lock()
        .map_err(|_| "应用状态已损坏".to_string())?;
    let library = inner
        .library
        .as_mut()
        .ok_or_else(|| "请先选择库目录".to_string())?;
    db::reorder_collections(&mut library.conn, &ordered_ids)
}

#[tauri::command]
fn set_collection_cover(
    state: State<'_, AppState>,
    collection_id: i64,
    archive_id: Option<i64>,
) -> Result<Collection, String> {
    let inner = state
        .inner
        .lock()
        .map_err(|_| "应用状态已损坏".to_string())?;
    let library = inner
        .library
        .as_ref()
        .ok_or_else(|| "请先选择库目录".to_string())?;
    db::set_collection_cover(&library.conn, collection_id, archive_id)
}

#[tauri::command]
fn add_archives_to_collection(
    state: State<'_, AppState>,
    collection_id: i64,
    archive_ids: Vec<i64>,
) -> Result<usize, String> {
    let mut inner = state
        .inner
        .lock()
        .map_err(|_| "应用状态已损坏".to_string())?;
    let library = inner
        .library
        .as_mut()
        .ok_or_else(|| "请先选择库目录".to_string())?;
    db::add_archives_to_collection(&mut library.conn, collection_id, &archive_ids)
}

#[tauri::command]
async fn open_reader(
    state: State<'_, AppState>,
    archive_id: i64,
) -> Result<ReaderInfo, String> {
    let (root, source, bookmarks) = {
        let inner = state
            .inner
            .lock()
            .map_err(|_| "应用状态已损坏".to_string())?;
        let library = inner
            .library
            .as_ref()
            .ok_or_else(|| "请先选择库目录".to_string())?;
        (
            library.root.clone(),
            db::archive_source(&library.conn, archive_id)?,
            db::reader_bookmarks(&library.conn, archive_id)?,
        )
    };

    if source.missing {
        return Err("文件缺失，无法阅读".to_string());
    }

    let path = fsutil::join_rel(&root, &source.rel_path)
        .ok_or_else(|| "画集文件路径无效".to_string())?;

    tauri::async_runtime::spawn_blocking(move || {
        let mut info = reader::reader_info(&path, source.last_page)?;
        info.bookmarks = bookmarks;
        Ok::<_, String>(info)
    })
    .await
    .map_err(|error| format!("阅读任务异常结束: {error}"))?
}

#[tauri::command]
async fn read_reader_page(
    state: State<'_, AppState>,
    archive_id: i64,
    index: i64,
) -> Result<ReaderPage, String> {
    let (root, source) = {
        let inner = state
            .inner
            .lock()
            .map_err(|_| "应用状态已损坏".to_string())?;
        let library = inner
            .library
            .as_ref()
            .ok_or_else(|| "请先选择库目录".to_string())?;
        (
            library.root.clone(),
            db::archive_source(&library.conn, archive_id)?,
        )
    };

    if source.missing {
        return Err("文件缺失，无法阅读".to_string());
    }

    let path = fsutil::join_rel(&root, &source.rel_path)
        .ok_or_else(|| "画集文件路径无效".to_string())?;

    tauri::async_runtime::spawn_blocking(move || reader::read_page(&path, index))
        .await
        .map_err(|error| format!("读取页面异常结束: {error}"))?
}

#[tauri::command]
async fn read_reader_thumbnail(
    state: State<'_, AppState>,
    archive_id: i64,
    index: i64,
    max_edge: Option<u32>,
) -> Result<ReaderPage, String> {
    let (root, source) = {
        let inner = state
            .inner
            .lock()
            .map_err(|_| "应用状态已损坏".to_string())?;
        let library = inner
            .library
            .as_ref()
            .ok_or_else(|| "请先选择库目录".to_string())?;
        (
            library.root.clone(),
            db::archive_source(&library.conn, archive_id)?,
        )
    };

    if source.missing {
        return Err("文件缺失，无法阅读".to_string());
    }

    let path = fsutil::join_rel(&root, &source.rel_path)
        .ok_or_else(|| "画集文件路径无效".to_string())?;

    tauri::async_runtime::spawn_blocking(move || reader::read_thumbnail(&path, index, max_edge))
        .await
        .map_err(|error| format!("读取缩略图异常结束: {error}"))?
}

#[tauri::command]
fn set_reading_progress(
    state: State<'_, AppState>,
    archive_id: i64,
    page: i64,
) -> Result<(), String> {
    let inner = state
        .inner
        .lock()
        .map_err(|_| "应用状态已损坏".to_string())?;
    let library = inner
        .library
        .as_ref()
        .ok_or_else(|| "请先选择库目录".to_string())?;
    db::set_reading_progress(&library.conn, archive_id, page)
}

#[tauri::command]
fn add_reader_bookmark(
    state: State<'_, AppState>,
    archive_id: i64,
    page: i64,
) -> Result<ReaderBookmark, String> {
    let inner = state
        .inner
        .lock()
        .map_err(|_| "应用状态已损坏".to_string())?;
    let library = inner
        .library
        .as_ref()
        .ok_or_else(|| "请先选择库目录".to_string())?;
    db::add_reader_bookmark(&library.conn, archive_id, page)
}

#[tauri::command]
fn remove_reader_bookmark(
    state: State<'_, AppState>,
    archive_id: i64,
    bookmark_id: i64,
) -> Result<(), String> {
    let inner = state
        .inner
        .lock()
        .map_err(|_| "应用状态已损坏".to_string())?;
    let library = inner
        .library
        .as_ref()
        .ok_or_else(|| "请先选择库目录".to_string())?;
    db::remove_reader_bookmark(&library.conn, archive_id, bookmark_id)
}

#[tauri::command]
fn query_archives(
    app: AppHandle,
    state: State<'_, AppState>,
    query: ArchiveQuery,
) -> Result<ArchivePage, String> {
    let inner = state
        .inner
        .lock()
        .map_err(|_| "应用状态已损坏".to_string())?;
    let library = inner
        .library
        .as_ref()
        .ok_or_else(|| "请先选择库目录".to_string())?;

    let covers_dir = paths::covers_dir(&app, &library.library_id)?;

    library::query_archives(&library.conn, &library.root, &covers_dir, &query)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let state = AppState::new(app.handle());
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_library_state,
            open_library,
            import_paths,
            scan_library,
            query_archives,
            set_rating,
            set_note,
            delete_archive,
            restore_archive,
            purge_archive,
            list_deleted_archives,
            list_tags,
            list_tag_categories,
            create_tag_category,
            rename_tag_category,
            move_tag_category,
            delete_tag_category,
            set_tag_category,
            list_deleted_tags,
            list_archive_tags,
            add_tag,
            remove_tag,
            update_tag,
            set_tag_translations,
            merge_tags,
            delete_tag,
            restore_tag,
            purge_tag,
            tag_graph,
            list_collections,
            create_collection,
            rename_collection,
            delete_collection,
            list_deleted_collections,
            restore_collection,
            purge_collection,
            reorder_collections,
            set_collection_cover,
            add_archives_to_collection,
            list_archive_collections,
            set_archive_collection,
            open_reader,
            read_reader_page,
            read_reader_thumbnail,
            set_reading_progress,
            add_reader_bookmark,
            remove_reader_bookmark
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests;

