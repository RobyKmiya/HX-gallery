mod anim;
mod archive;
mod asset_protocol;
mod browser;
mod container;
mod d2;
pub mod db;
pub mod fsutil;
pub mod importer;
pub mod instance;
pub mod library;
pub mod loose_images;
pub mod models;
mod pagecache;
mod paths;
pub mod queue;
mod reader;
mod release;
pub mod series;
pub mod translate;
pub mod userscripts;

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

use models::{
    ArchivePage, ArchiveQuery, BulkTagReport, Collection, ImportOptions, ImportReport, LibraryState,
    Queue, QueueFillOptions, QueueFillReport, ReaderBookmark, ReaderInfo, ReaderPage, ReaderThumb,
    ScanReport, SeriesSummary, StageItem, StageReport, Tag, TagCategory, TagDraft, TagGraph,
    TagTranslationInput, TrashedArchive,
};

/// 一次导入最多接受多少个入口路径，避免单次 IPC 传入超大数组。
const MAX_IMPORT_INPUTS: usize = 20_000;

/// 导入面板里至多给这么多本数页数（打开 zip 中央目录，单本几十毫秒）。
const STAGE_PAGE_COUNT_LIMIT: usize = 80;
/// 导入面板里至多给这么多本做「同名同大小」查重（每本一条 SQL）。
const STAGE_DUPLICATE_LIMIT: usize = 300;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct AppSettings {
    library_path: Option<String>,
}

struct AppInner {
    library: Option<db::LibraryContext>,
    last_library_path: Option<String>,
    startup_error: Option<String>,
}

/// 应用状态。
///
/// 用 `Arc<Mutex<..>>` 而不是裸 `Mutex`：Tauri 命令需要把状态搬进 `spawn_blocking`
/// 的闭包里执行，`State<'_, _>` 带生命周期，只有可克隆的状态句柄能跨线程使用。
pub struct AppState {
    inner: Arc<Mutex<AppInner>>,
}

/// 给 loose:// 协议与散图命令用的（库根, 库数据库文件路径）。
pub(crate) fn library_root_and_db(state: &AppState) -> Option<(PathBuf, PathBuf)> {
    let guard = state.inner.lock().ok()?;
    let library = guard.library.as_ref()?;
    // 库数据库在 <库根>/.gallery/library.db（不是库根下的 library.db）。
    // 这里拼错会让散图协议按 id 查记录时永远查不到 → 每张图 404（2026-11-22 全裂图事故）。
    let db_path = library.root.join(".gallery").join("library.db");
    Some((library.root.clone(), db_path))
}

impl Clone for AppState {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
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
                        eprintln!("opened library at {}", library.root.to_string_lossy());
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
            inner: Arc::new(Mutex::new(inner)),
        }
    }

    fn lock(&self) -> Result<MutexGuard<'_, AppInner>, String> {
        self.inner
            .lock()
            .map_err(|_| "应用状态已损坏，请重启应用".to_string())
    }
}

/// 导入 / 扫描的进程内互斥。
///
/// 此前连点两次「导入」或导入中再点「扫描」会各开一条连接同时写库，
/// 轻则大量 `database is locked`，重则扫描把刚导入的文件当成缺失。
/// CLI 是独立进程，靠 SQLite 的写锁 + busy_timeout 协调。
static MAINTENANCE: Mutex<()> = Mutex::new(());

fn try_maintenance() -> Result<MutexGuard<'static, ()>, String> {
    MAINTENANCE
        .try_lock()
        .map_err(|_| "已有导入或扫描正在进行，请等它完成".to_string())
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
        eprintln!("failed to resolve settings path");
        return AppSettings::default();
    };
    let Ok(raw) = fs::read_to_string(&path) else {
        return AppSettings::default();
    };
    match serde_json::from_str(&raw) {
        Ok(settings) => settings,
        Err(error) => {
            // 静默退回默认值会让「上次的库」凭空消失，至少留下线索。
            eprintln!("settings.json 解析失败（{}）: {error}", path.to_string_lossy());
            AppSettings::default()
        }
    }
}

fn write_settings(app: &AppHandle, settings: &AppSettings) {
    let Ok(path) = settings_path(app) else {
        eprintln!("failed to resolve settings path");
        return;
    };
    let raw = match serde_json::to_string_pretty(settings) {
        Ok(raw) => raw,
        Err(error) => {
            eprintln!("failed to serialize settings: {error}");
            return;
        }
    };

    // 先写同目录临时文件再 rename：避免写到一半（崩溃/断电）留下半截 JSON，
    // 下次启动读失败后又被静默当成「从未选过库」。
    let temp = path.with_extension("json.tmp");
    if let Err(error) = fs::write(&temp, raw) {
        eprintln!("failed to write settings: {error}");
        return;
    }
    if let Err(error) = fs::rename(&temp, &path) {
        eprintln!("failed to replace settings: {error}");
        let _ = fs::remove_file(&temp);
    }
}

fn build_library_state(state: &AppState) -> Result<LibraryState, String> {
    let inner = state.lock()?;

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

fn with_library<T>(
    state: &AppState,
    op: impl FnOnce(&db::LibraryContext) -> Result<T, String>,
) -> Result<T, String> {
    let inner = state.lock()?;
    let library = inner
        .library
        .as_ref()
        .ok_or_else(|| "请先选择库目录".to_string())?;
    op(library)
}

fn with_library_mut<T>(
    state: &AppState,
    op: impl FnOnce(&mut db::LibraryContext) -> Result<T, String>,
) -> Result<T, String> {
    let mut inner = state.lock()?;
    let library = inner
        .library
        .as_mut()
        .ok_or_else(|| "请先选择库目录".to_string())?;
    op(library)
}

/// 在阻塞线程里执行一次库操作。
///
/// Tauri 2 里同步命令跑在主线程，而 `query_archives`、`tag_graph` 这类查询可能
/// 扫全表；一旦在主线程持锁执行，整个界面会直接冻结。所有碰数据库的命令都走这里。
async fn run_blocking<T, F>(state: State<'_, AppState>, op: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&AppState) -> Result<T, String> + Send + 'static,
{
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || op(&state))
        .await
        .map_err(|error| format!("后台任务异常结束: {error}"))?
}

/// 校验用户选的库目录：必须是已存在的绝对路径目录。
///
/// 原来只判空，`open_library` 会 `create_dir_all`，等于允许在任意路径下新建
/// `.gallery` 数据库目录。库目录由目录选择器给出，必然已存在，这里不放行「顺手创建」。
fn validate_library_dir(path: &str) -> Result<PathBuf, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("库目录不能为空".to_string());
    }

    let candidate = PathBuf::from(trimmed);
    if !candidate.is_absolute() {
        return Err("库目录必须是绝对路径".to_string());
    }
    if !candidate.is_dir() {
        return Err(format!(
            "目录不存在或不是文件夹：{}",
            candidate.to_string_lossy()
        ));
    }

    Ok(candidate)
}

#[tauri::command]
async fn get_library_state(state: State<'_, AppState>) -> Result<LibraryState, String> {
    run_blocking(state, build_library_state).await
}

#[tauri::command]
async fn open_library(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> Result<LibraryState, String> {
    let target = validate_library_dir(&path)?;
    let state_handle = state.inner().clone();

    tauri::async_runtime::spawn_blocking(move || {
        let library = db::open_library(&target)?;
        let display_path = library.root.to_string_lossy().to_string();

        {
            let mut inner = state_handle.lock()?;
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

        build_library_state(&state_handle)
    })
    .await
    .map_err(|error| format!("打开库任务异常结束: {error}"))?
}

#[tauri::command]
async fn import_paths(
    app: AppHandle,
    state: State<'_, AppState>,
    paths: Vec<String>,
    options: ImportOptions,
) -> Result<ImportReport, String> {
    if paths.is_empty() {
        return Err("没有要导入的路径".to_string());
    }
    if paths.len() > MAX_IMPORT_INPUTS {
        return Err(format!("一次最多导入 {MAX_IMPORT_INPUTS} 个路径"));
    }

    let (root, library_id) = {
        let inner = state.lock()?;
        let library = inner
            .library
            .as_ref()
            .ok_or_else(|| "请先选择库目录".to_string())?;
        (library.root.clone(), library.library_id.clone())
    };

    let input_paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();

    tauri::async_runtime::spawn_blocking(move || -> Result<ImportReport, String> {
        let _guard = try_maintenance()?;
        let covers_dir = paths::covers_dir(&app, &library_id)?;
        Ok(importer::import_blocking(
            &app,
            &root,
            &covers_dir,
            input_paths,
            options,
        ))
    })
    .await
    .map_err(|error| format!("导入任务异常结束: {error}"))?
}

/// 导入面板的暂存：把路径展开成压缩包清单，并给出大小、页数与「疑似重复」提示。
///
/// 这里刻意不做任何重活：不解压、不解码封面、不算内容哈希。面板只负责让用户
/// 在确认前看清「要进来什么」，真正的去重与封面生成仍在 `import_paths` 里完成。
#[tauri::command]
async fn stage_import_paths(
    state: State<'_, AppState>,
    paths: Vec<String>,
) -> Result<StageReport, String> {
    if paths.len() > MAX_IMPORT_INPUTS {
        return Err(format!("一次最多导入 {MAX_IMPORT_INPUTS} 个路径"));
    }
    if paths.is_empty() {
        return Ok(StageReport {
            items: Vec::new(),
            errors: Vec::new(),
            total_bytes: 0,
        });
    }

    let root = {
        let inner = state.lock()?;
        inner
            .library
            .as_ref()
            .ok_or_else(|| "请先选择库目录".to_string())?
            .root
            .clone()
    };
    let inputs: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();

    tauri::async_runtime::spawn_blocking(move || -> Result<StageReport, String> {
        // 只查重需要数据库，用工作连接（不迁移、不抢主连接）。
        let connection = db::open_worker_connection(&root)?;
        let (staged, mut errors) = importer::gather_inputs(&root, &inputs);

        // 一次拖几百本时，页数与查重都是「每本一次 IO / 一次查询」，会拖成十几秒的空白。
        // 超过阈值就先不显示（导入时该有的信息一样不少），保证面板立刻可用。
        let with_pages = staged.len() <= STAGE_PAGE_COUNT_LIMIT;
        let with_duplicates = staged.len() <= STAGE_DUPLICATE_LIMIT;

        let mut items = Vec::with_capacity(staged.len());
        let mut total_bytes = 0i64;

        for input in staged {
            let file_name = input
                .source
                .file_name()
                .map(|value| value.to_string_lossy().to_string())
                .unwrap_or_else(|| input.source.to_string_lossy().to_string());
            let stem = input
                .source
                .file_stem()
                .map(|value| value.to_string_lossy().to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| file_name.clone());
            let extension = input
                .source
                .extension()
                .map(|value| value.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();

            let (size_bytes, issue) = match fs::metadata(&input.source) {
                Ok(metadata) if metadata.is_file() => (metadata.len() as i64, input.issue),
                Ok(_) => (0, Some("不是普通文件".to_string())),
                Err(error) => (0, Some(format!("无法读取文件信息: {error}"))),
            };
            total_bytes += size_bytes.max(0);

            let page_count = if issue.is_none() && with_pages {
                archive::count_pages(&input.source).ok()
            } else {
                None
            };

            let duplicate = issue.is_none()
                && with_duplicates
                && db::has_similar_archive(&connection, &file_name, size_bytes).unwrap_or(false);

            items.push(StageItem {
                path: input.source.to_string_lossy().to_string(),
                file_name,
                stem,
                extension,
                size_bytes,
                page_count,
                duplicate,
                in_library: input.in_library,
                issue,
            });
        }

        // 前端展示顺序按文件名排，和导入顺序一致，用户核对时不会跳来跳去。
        items.sort_by(|left, right| {
            left.file_name
                .to_ascii_lowercase()
                .cmp(&right.file_name.to_ascii_lowercase())
        });

        Ok(StageReport {
            items,
            total_bytes,
            errors: std::mem::take(&mut errors),
        })
    })
    .await
    .map_err(|error| format!("准备导入清单异常结束: {error}"))?
}

#[tauri::command]
async fn scan_library(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<ScanReport, String> {
    let (root, library_id) = {
        let inner = state.lock()?;
        let library = inner
            .library
            .as_ref()
            .ok_or_else(|| "请先选择库目录".to_string())?;
        (library.root.clone(), library.library_id.clone())
    };

    tauri::async_runtime::spawn_blocking(move || -> Result<ScanReport, String> {
        let _guard = try_maintenance()?;
        Ok(library::scan_blocking(&app, &root, &library_id))
    })
    .await
    .map_err(|error| format!("扫描任务异常结束: {error}"))?
}

#[tauri::command]
async fn set_rating(
    state: State<'_, AppState>,
    id: i64,
    rating: i64,
) -> Result<i64, String> {
    run_blocking(state, move |state| {
        with_library(state, |library| {
            db::set_rating(&library.conn, id, rating)?;
            Ok(rating)
        })
    })
    .await
}

#[tauri::command]
async fn list_tags(state: State<'_, AppState>) -> Result<Vec<Tag>, String> {
    run_blocking(state, |state| {
        with_library(state, |library| db::list_tags(&library.conn))
    })
    .await
}

#[tauri::command]
async fn list_tag_categories(state: State<'_, AppState>) -> Result<Vec<TagCategory>, String> {
    run_blocking(state, |state| {
        with_library(state, |library| db::list_tag_categories(&library.conn))
    })
    .await
}

#[tauri::command]
async fn create_tag_category(
    state: State<'_, AppState>,
    parent_path: Option<String>,
    name: String,
) -> Result<TagCategory, String> {
    run_blocking(state, move |state| {
        with_library(state, |library| {
            db::create_tag_category(&library.conn, parent_path.as_deref(), &name)
        })
    })
    .await
}

#[tauri::command]
async fn rename_tag_category(
    state: State<'_, AppState>,
    path: String,
    new_name: String,
) -> Result<Vec<TagCategory>, String> {
    run_blocking(state, move |state| {
        with_library_mut(state, |library| {
            db::rename_tag_category(&mut library.conn, &path, &new_name)
        })
    })
    .await
}

#[tauri::command]
async fn move_tag_category(
    state: State<'_, AppState>,
    path: String,
    new_parent_path: Option<String>,
) -> Result<Vec<TagCategory>, String> {
    run_blocking(state, move |state| {
        with_library_mut(state, |library| {
            db::move_tag_category(&mut library.conn, &path, new_parent_path.as_deref())
        })
    })
    .await
}

#[tauri::command]
async fn delete_tag_category(
    state: State<'_, AppState>,
    path: String,
) -> Result<Vec<TagCategory>, String> {
    run_blocking(state, move |state| {
        with_library_mut(state, |library| {
            db::delete_tag_category(&mut library.conn, &path)
        })
    })
    .await
}

#[tauri::command]
async fn set_tag_category(
    state: State<'_, AppState>,
    tag_id: i64,
    category: Option<String>,
) -> Result<Tag, String> {
    run_blocking(state, move |state| {
        with_library(state, |library| {
            db::set_tag_category(&library.conn, tag_id, category.as_deref())
        })
    })
    .await
}

#[tauri::command]
async fn list_archive_tags(
    state: State<'_, AppState>,
    archive_id: i64,
) -> Result<Vec<Tag>, String> {
    run_blocking(state, move |state| {
        with_library(state, |library| db::archive_tags(&library.conn, archive_id))
    })
    .await
}

#[tauri::command]
async fn add_tag(
    state: State<'_, AppState>,
    archive_id: i64,
    name: String,
) -> Result<Tag, String> {
    run_blocking(state, move |state| {
        with_library(state, |library| {
            db::add_tag(&library.conn, archive_id, &name)
        })
    })
    .await
}

/// 批量导入标签（一次事务）。返回成功/失败明细，单条失败不影响其余。
#[tauri::command]
async fn add_tags_bulk(
    state: State<'_, AppState>,
    archive_id: i64,
    tags: Vec<TagDraft>,
) -> Result<BulkTagReport, String> {
    run_blocking(state, move |state| {
        with_library(state, |library| {
            db::add_tags_bulk(&library.conn, archive_id, &tags)
        })
    })
    .await
}

#[tauri::command]
async fn remove_tag(
    state: State<'_, AppState>,
    archive_id: i64,
    tag_id: i64,
) -> Result<(), String> {
    run_blocking(state, move |state| {
        with_library(state, |library| {
            db::remove_tag(&library.conn, archive_id, tag_id)
        })
    })
    .await
}

#[tauri::command]
async fn update_tag(
    state: State<'_, AppState>,
    tag_id: i64,
    name: String,
    translated_name: Option<String>,
) -> Result<Tag, String> {
    run_blocking(state, move |state| {
        with_library(state, |library| {
            db::update_tag(
                &library.conn,
                tag_id,
                &name,
                translated_name.as_deref(),
            )
        })
    })
    .await
}

#[tauri::command]
async fn delete_tag(state: State<'_, AppState>, tag_id: i64) -> Result<(), String> {
    run_blocking(state, move |state| {
        with_library(state, |library| db::delete_tag(&library.conn, tag_id))
    })
    .await
}

#[tauri::command]
async fn tag_graph(
    state: State<'_, AppState>,
    min_weight: Option<i64>,
    limit: Option<usize>,
) -> Result<TagGraph, String> {
    run_blocking(state, move |state| {
        with_library(state, |library| {
            db::tag_graph(
                &library.conn,
                min_weight.unwrap_or(1),
                limit.unwrap_or(300).clamp(1, 2_000),
            )
        })
    })
    .await
}

#[tauri::command]
async fn delete_archive(state: State<'_, AppState>, archive_id: i64) -> Result<(), String> {
    run_blocking(state, move |state| {
        with_library(state, |library| {
            db::delete_archive(&library.conn, archive_id)
        })
    })
    .await
}

#[tauri::command]
async fn restore_archive(state: State<'_, AppState>, archive_id: i64) -> Result<(), String> {
    run_blocking(state, move |state| {
        with_library(state, |library| {
            db::restore_archive(&library.conn, archive_id)
        })
    })
    .await
}

#[tauri::command]
async fn purge_archive(
    app: AppHandle,
    state: State<'_, AppState>,
    archive_id: i64,
) -> Result<(), String> {
    run_blocking(state, move |state| {
        with_library(state, |library| {
            let covers_dir = paths::covers_dir(&app, &library.library_id)?;
            let cover_key = db::purge_archive(&library.conn, archive_id)?;
            if let Some(key) = cover_key {
                // 先确认没有其它记录（含回收站）还在用这个封面，再删；
                // 同时校验 key 形态，避免被篡改的 cover_key 拼出缓存目录之外的路径。
                if archive::is_cover_key(&key) && !db::cover_key_in_use(&library.conn, &key)? {
                    let _ = fs::remove_file(covers_dir.join(&key));
                }
            }
            Ok(())
        })
    })
    .await
}

#[tauri::command]
async fn list_deleted_archives(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<TrashedArchive>, String> {
    run_blocking(state, move |state| {
        with_library(state, |library| {
            let covers_dir = paths::covers_dir(&app, &library.library_id)?;
            db::list_deleted_archives(&library.conn, Some(&covers_dir))
        })
    })
    .await
}

#[tauri::command]
async fn list_deleted_tags(state: State<'_, AppState>) -> Result<Vec<Tag>, String> {
    run_blocking(state, |state| {
        with_library(state, |library| db::list_deleted_tags(&library.conn))
    })
    .await
}

#[tauri::command]
async fn restore_tag(state: State<'_, AppState>, tag_id: i64) -> Result<Tag, String> {
    run_blocking(state, move |state| {
        with_library(state, |library| db::restore_tag(&library.conn, tag_id))
    })
    .await
}

#[tauri::command]
async fn purge_tag(state: State<'_, AppState>, tag_id: i64) -> Result<(), String> {
    run_blocking(state, move |state| {
        with_library(state, |library| db::purge_tag(&library.conn, tag_id))
    })
    .await
}

#[tauri::command]
async fn merge_tags(
    state: State<'_, AppState>,
    source_ids: Vec<i64>,
    target_id: i64,
) -> Result<Tag, String> {
    run_blocking(state, move |state| {
        with_library_mut(state, |library| {
            db::merge_tags(&mut library.conn, &source_ids, target_id)
        })
    })
    .await
}

#[tauri::command]
async fn set_tag_translations(
    state: State<'_, AppState>,
    updates: Vec<TagTranslationInput>,
) -> Result<usize, String> {
    run_blocking(state, move |state| {
        with_library_mut(state, |library| {
            db::set_tag_translations_batch(&mut library.conn, &updates)
        })
    })
    .await
}

#[tauri::command]
async fn set_note(
    state: State<'_, AppState>,
    id: i64,
    note: Option<String>,
) -> Result<Option<String>, String> {
    run_blocking(state, move |state| {
        with_library(state, |library| {
            db::set_note(&library.conn, id, note.as_deref())
        })
    })
    .await
}

#[tauri::command]
async fn list_collections(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<Collection>, String> {
    run_blocking(state, move |state| {
        with_library(state, |library| {
            let covers_dir = paths::covers_dir(&app, &library.library_id)?;
            db::list_collections(&library.conn, Some(&covers_dir))
        })
    })
    .await
}

#[tauri::command]
async fn create_collection(
    state: State<'_, AppState>,
    name: String,
) -> Result<Collection, String> {
    run_blocking(state, move |state| {
        with_library(state, |library| {
            db::create_collection(&library.conn, &name)
        })
    })
    .await
}

#[tauri::command]
async fn list_archive_collections(
    state: State<'_, AppState>,
    archive_id: i64,
) -> Result<Vec<i64>, String> {
    run_blocking(state, move |state| {
        with_library(state, |library| {
            db::archive_collections(&library.conn, archive_id)
        })
    })
    .await
}

#[tauri::command]
async fn set_archive_collection(
    state: State<'_, AppState>,
    archive_id: i64,
    collection_id: i64,
    member: bool,
) -> Result<(), String> {
    run_blocking(state, move |state| {
        with_library(state, |library| {
            db::set_archive_collection(&library.conn, archive_id, collection_id, member)
        })
    })
    .await
}

#[tauri::command]
async fn rename_collection(
    state: State<'_, AppState>,
    collection_id: i64,
    name: String,
) -> Result<Collection, String> {
    run_blocking(state, move |state| {
        with_library(state, |library| {
            db::rename_collection(&library.conn, collection_id, &name)
        })
    })
    .await
}

#[tauri::command]
async fn delete_collection(
    state: State<'_, AppState>,
    collection_id: i64,
) -> Result<(), String> {
    run_blocking(state, move |state| {
        with_library(state, |library| {
            db::delete_collection(&library.conn, collection_id)
        })
    })
    .await
}

#[tauri::command]
async fn list_deleted_collections(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<Collection>, String> {
    run_blocking(state, move |state| {
        with_library(state, |library| {
            let covers_dir = paths::covers_dir(&app, &library.library_id)?;
            db::list_deleted_collections(&library.conn, Some(&covers_dir))
        })
    })
    .await
}

#[tauri::command]
async fn restore_collection(
    app: AppHandle,
    state: State<'_, AppState>,
    collection_id: i64,
) -> Result<Collection, String> {
    run_blocking(state, move |state| {
        with_library(state, |library| {
            let covers_dir = paths::covers_dir(&app, &library.library_id)?;
            db::restore_collection(&library.conn, collection_id, Some(&covers_dir))
        })
    })
    .await
}

#[tauri::command]
async fn purge_collection(
    state: State<'_, AppState>,
    collection_id: i64,
) -> Result<(), String> {
    run_blocking(state, move |state| {
        with_library(state, |library| {
            db::purge_collection(&library.conn, collection_id)
        })
    })
    .await
}

#[tauri::command]
async fn reorder_collections(
    state: State<'_, AppState>,
    ordered_ids: Vec<i64>,
) -> Result<(), String> {
    run_blocking(state, move |state| {
        with_library_mut(state, |library| {
            db::reorder_collections(&mut library.conn, &ordered_ids)
        })
    })
    .await
}

#[tauri::command]
async fn set_collection_cover(
    state: State<'_, AppState>,
    collection_id: i64,
    archive_id: Option<i64>,
) -> Result<Collection, String> {
    run_blocking(state, move |state| {
        with_library(state, |library| {
            db::set_collection_cover(&library.conn, collection_id, archive_id)
        })
    })
    .await
}

#[tauri::command]
async fn add_archives_to_collection(
    state: State<'_, AppState>,
    collection_id: i64,
    archive_ids: Vec<i64>,
) -> Result<usize, String> {
    run_blocking(state, move |state| {
        with_library_mut(state, |library| {
            db::add_archives_to_collection(&mut library.conn, collection_id, &archive_ids)
        })
    })
    .await
}

/// 把画集 id 解析成磁盘上的压缩包路径。
///
/// 阅读相关的命令都要先走这一步：路径只从数据库里取，配合 `join_rel`
/// 的校验，避免把 IPC 传来的字符串当成文件路径。
fn archive_file_path(state: &AppState, archive_id: i64) -> Result<PathBuf, String> {
    let (root, source) = {
        let inner = state.lock()?;
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

    fsutil::join_rel(&root, &source.rel_path).ok_or_else(|| "画集文件路径无效".to_string())
}

#[tauri::command]
async fn open_reader(
    state: State<'_, AppState>,
    archive_id: i64,
) -> Result<ReaderInfo, String> {
    let (path, last_page, bookmarks) = {
        let inner = state.lock()?;
        let library = inner
            .library
            .as_ref()
            .ok_or_else(|| "请先选择库目录".to_string())?;
        let source = db::archive_source(&library.conn, archive_id)?;
        if source.missing {
            return Err("文件缺失，无法阅读".to_string());
        }
        let path = fsutil::join_rel(&library.root, &source.rel_path)
            .ok_or_else(|| "画集文件路径无效".to_string())?;
        // 打开即计一次。放在真正进入阅读器时自增，预览/翻页都不算额外一次。
        db::record_read(&library.conn, archive_id)?;
        (
            path,
            source.last_page,
            db::reader_bookmarks(&library.conn, archive_id)?,
        )
    };

    tauri::async_runtime::spawn_blocking(move || {
        reader::remember_archive(archive_id, &path);
        let mut info = reader::reader_info(&path, last_page)?;
        info.bookmarks = bookmarks;
        Ok::<_, String>(info)
    })
    .await
    .map_err(|error| format!("阅读任务异常结束: {error}"))?
}

/// 预热一页并返回元信息。
///
/// 图片字节留在后端缓存里，由 `gallery://` 协议取用；这里负责把
/// 「解压 + 解码 + TIFF 转码」放到阻塞线程上，界面不会因此停顿。
#[tauri::command]
async fn read_reader_page(
    state: State<'_, AppState>,
    archive_id: i64,
    index: i64,
    current: Option<bool>,
) -> Result<ReaderPage, String> {
    let pin = current.unwrap_or(false);
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let path = archive_file_path(&state, archive_id)?;
        reader::remember_archive(archive_id, &path);
        reader::read_page(&path, index, pin)
    })
    .await
    .map_err(|error| format!("读取页面异常结束: {error}"))?
}

/// 预热某页的动画元数据（帧数/总时长），供播放模式决定动图页停留多久。
#[tauri::command]
async fn read_anim_meta(
    state: State<'_, AppState>,
    archive_id: i64,
    index: i64,
) -> Result<anim::AnimMeta, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let path = archive_file_path(&state, archive_id)?;
        reader::remember_archive(archive_id, &path);
        anim::anim_meta(&path, index)
    })
    .await
    .map_err(|error| format!("读取动画信息异常结束: {error}"))?
}

#[tauri::command]
async fn read_reader_thumbnail(
    state: State<'_, AppState>,
    archive_id: i64,
    index: i64,
    max_edge: Option<u32>,
) -> Result<ReaderThumb, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let path = archive_file_path(&state, archive_id)?;
        reader::read_thumbnail(&path, index, max_edge)
    })
    .await
    .map_err(|error| format!("读取缩略图异常结束: {error}"))?
}

#[tauri::command]
async fn set_reading_progress(
    state: State<'_, AppState>,
    archive_id: i64,
    page: i64,
) -> Result<(), String> {
    run_blocking(state, move |state| {
        with_library(state, |library| {
            db::set_reading_progress(&library.conn, archive_id, page)
        })
    })
    .await
}

#[tauri::command]
async fn list_queues(state: State<'_, AppState>) -> Result<Vec<Queue>, String> {
    run_blocking(state, move |state| {
        with_library(state, |library| queue::list_queues(&library.conn))
    })
    .await
}

#[tauri::command]
async fn fill_queue(
    state: State<'_, AppState>,
    name: String,
    options: QueueFillOptions,
) -> Result<QueueFillReport, String> {
    run_blocking(state, move |state| {
        with_library_mut(state, |library| {
            queue::fill_queue(&mut library.conn, &name, &options, None)
        })
    })
    .await
}

#[tauri::command]
async fn delete_queue(state: State<'_, AppState>, queue_id: i64) -> Result<(), String> {
    run_blocking(state, move |state| {
        with_library(state, |library| queue::delete_queue(&library.conn, queue_id))
    })
    .await
}

#[tauri::command]
async fn add_to_queue(
    state: State<'_, AppState>,
    queue_id: i64,
    archive_id: i64,
) -> Result<bool, String> {
    run_blocking(state, move |state| {
        with_library(state, |library| {
            queue::add_to_queue(&library.conn, queue_id, archive_id)
        })
    })
    .await
}

#[tauri::command]
async fn remove_from_queue(
    state: State<'_, AppState>,
    queue_id: i64,
    archive_id: i64,
) -> Result<(), String> {
    run_blocking(state, move |state| {
        with_library(state, |library| {
            queue::remove_from_queue(&library.conn, queue_id, archive_id)
        })
    })
    .await
}

#[tauri::command]
async fn list_queue_archives(
    state: State<'_, AppState>,
    queue_id: i64,
) -> Result<Vec<i64>, String> {
    run_blocking(state, move |state| {
        with_library(state, |library| queue::queue_archives(&library.conn, queue_id))
    })
    .await
}

#[tauri::command]
async fn create_queue(
    state: State<'_, AppState>,
    name: String,
) -> Result<Queue, String> {
    run_blocking(state, move |state| {
        with_library_mut(state, |library| {
            queue::create_queue(&mut library.conn, &name)
        })
    })
    .await
}

#[tauri::command]
async fn add_many_to_queue(
    state: State<'_, AppState>,
    queue_id: i64,
    archive_ids: Vec<i64>,
) -> Result<i64, String> {
    run_blocking(state, move |state| {
        with_library_mut(state, |library| {
            queue::add_many_to_queue(&mut library.conn, queue_id, &archive_ids)
        })
    })
    .await
}

#[tauri::command]
async fn move_queue_archive(
    state: State<'_, AppState>,
    queue_id: i64,
    archive_id: i64,
    direction: i64,
) -> Result<(), String> {
    run_blocking(state, move |state| {
        with_library_mut(state, |library| {
            queue::move_queue_archive(&mut library.conn, queue_id, archive_id, direction)
        })
    })
    .await
}

#[tauri::command]
async fn set_tag_weight(
    state: State<'_, AppState>,
    tag_id: i64,
    weight: i64,
) -> Result<Tag, String> {
    run_blocking(state, move |state| {
        with_library(state, |library| {
            db::set_tag_weight(&library.conn, tag_id, weight)
        })
    })
    .await
}

#[tauri::command]
async fn add_reader_bookmark(
    state: State<'_, AppState>,
    archive_id: i64,
    page: i64,
) -> Result<ReaderBookmark, String> {
    run_blocking(state, move |state| {
        with_library(state, |library| {
            db::add_reader_bookmark(&library.conn, archive_id, page)
        })
    })
    .await
}

#[tauri::command]
async fn remove_reader_bookmark(
    state: State<'_, AppState>,
    archive_id: i64,
    bookmark_id: i64,
) -> Result<(), String> {
    run_blocking(state, move |state| {
        with_library(state, |library| {
            db::remove_reader_bookmark(&library.conn, archive_id, bookmark_id)
        })
    })
    .await
}

/// 读取「退出后自动发布」的设置与上次结果。
#[tauri::command]
fn get_release_state(app: AppHandle) -> serde_json::Value {
    let settings = release::load_settings(&app);
    let status = release::load_status(&app);
    serde_json::json!({
        "sourceDir": settings.source_dir,
        "autoOnExit": settings.auto_on_exit,
        "relaunchAfter": settings.relaunch_after,
        "status": status,
        "readiness": release::release_readiness(&app, &settings),
    })
}

/// 保存设置。`sourceDir` 会先校验（必须含 `scripts/release.mjs`）。
#[tauri::command]
fn save_release_settings(
    app: AppHandle,
    source_dir: Option<String>,
    auto_on_exit: bool,
    relaunch_after: bool,
) -> Result<serde_json::Value, String> {
    let normalized = match source_dir.as_deref() {
        Some(raw) if !raw.trim().is_empty() => {
            Some(release::validate_source_dir(raw)?.to_string_lossy().to_string())
        }
        _ => None,
    };

    // 开了自动发布却没给源码目录，等于给自己埋一个「退出后什么都没发生」的坑。
    if auto_on_exit && normalized.is_none() {
        return Err("开启自动发布前，请先选择画匣源码目录".to_string());
    }

    let settings = release::ReleaseSettings {
        source_dir: normalized,
        auto_on_exit,
        relaunch_after,
    };
    release::save_settings(&app, &settings)?;

    let status = release::load_status(&app);
    Ok(serde_json::json!({
        "sourceDir": settings.source_dir,
        "autoOnExit": settings.auto_on_exit,
        "relaunchAfter": settings.relaunch_after,
        "status": status,
        "readiness": release::release_readiness(&app, &settings),
    }))
}

/// 安排一次「退出后发布」，但**不退出**——由前端确认后再调 `quit_app`。
///
/// 分开两步是为了让前端有机会先把「正在更新」的提示画出来、把未落盘的进度写完。
#[tauri::command]
fn schedule_release(app: AppHandle) -> Result<serde_json::Value, String> {
    let settings = release::load_settings(&app);
    let (script, log) = release::schedule_of(&app, &settings)?;
    Ok(serde_json::json!({ "scriptPath": script, "logPath": log }))
}

/// 真退出：先确保脚本已经被拉起来，再结束进程。
#[tauri::command]
fn quit_for_release(app: AppHandle) -> Result<(), String> {
    let settings = release::load_settings(&app);
    // 排期只是写几个小文件，同步做掉即可（出错还能回给前端）。
    let (script, _) = release::schedule_of(&app, &settings)?;

    // 拉起脚本和退出放后台：explorer 启动 + 400ms 等待都别卡在窗口上，
    // 先把主窗藏掉，用户看到的就是「点了就关」（与关窗同一条体验要求）。
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
    let handle = app.clone();
    std::thread::spawn(move || {
        let _ = release::launch_detached(std::path::Path::new(&script));
        // 给脚本一点时间启动，然后退出。脚本那边还会再等一次进程消失。
        std::thread::sleep(std::time::Duration::from_millis(400));
        let _ = handle.exit(0);
    });
    Ok(())
}

/// 阅读器沉浸模式：连系统标题栏一起去掉（真无边框），退出阅读时再恢复。
///
/// 最大化状态下直接切装饰会让客户区尺寸对不上（内容会跑到屏幕外一截），
/// 所以先取消最大化、切装饰、再最大化一次让系统重新算布局。
#[tauri::command]
fn set_window_decorations(window: tauri::WebviewWindow, decorated: bool) -> Result<(), String> {
    let maximized = window.is_maximized().unwrap_or(false);
    if maximized {
        let _ = window.unmaximize();
    }
    window
        .set_decorations(decorated)
        .map_err(|e| format!("切换窗口边框失败: {e}"))?;
    if maximized {
        let _ = window.maximize();
    }
    Ok(())
}

// ---------- 合集（连载盒子） ----------

#[tauri::command]
async fn list_series(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<SeriesSummary>, String> {
    run_blocking(state, move |state| {
        with_library(state, |library| {
            let covers_dir = paths::covers_dir(&app, &library.library_id)?;
            series::list_series(&library.conn, &covers_dir)
        })
    })
    .await
}

#[tauri::command]
async fn create_series(
    app: AppHandle,
    state: State<'_, AppState>,
    name: String,
    archive_ids: Vec<i64>,
) -> Result<i64, String> {
    run_blocking(state, move |state| {
        with_library(state, |library| {
            let id = series::create_series(&library.conn, &name, &archive_ids)?;
            let _ = paths::covers_dir(&app, &library.library_id);
            Ok(id)
        })
    })
    .await
}

#[tauri::command]
async fn update_series(
    state: State<'_, AppState>,
    series_id: i64,
    name: String,
    note: Option<String>,
) -> Result<(), String> {
    run_blocking(state, move |state| {
        with_library(state, |library| {
            series::update_series(&library.conn, series_id, &name, note.as_deref())
        })
    })
    .await
}

#[tauri::command]
async fn dissolve_series(state: State<'_, AppState>, series_id: i64) -> Result<(), String> {
    run_blocking(state, move |state| {
        with_library(state, |library| series::dissolve_series(&library.conn, series_id))
    })
    .await
}

#[tauri::command]
async fn series_set_cover(
    state: State<'_, AppState>,
    series_id: i64,
    archive_id: Option<i64>,
) -> Result<(), String> {
    run_blocking(state, move |state| {
        with_library(state, |library| {
            series::set_cover(&library.conn, series_id, archive_id)
        })
    })
    .await
}

#[tauri::command]
async fn series_reorder(
    state: State<'_, AppState>,
    series_id: i64,
    archive_ids: Vec<i64>,
) -> Result<(), String> {
    run_blocking(state, move |state| {
        with_library(state, |library| {
            series::reorder(&library.conn, series_id, &archive_ids)
        })
    })
    .await
}

#[tauri::command]
async fn series_set_label(
    state: State<'_, AppState>,
    series_id: i64,
    archive_id: i64,
    label: Option<String>,
) -> Result<(), String> {
    run_blocking(state, move |state| {
        with_library(state, |library| {
            series::set_label(&library.conn, series_id, archive_id, label.as_deref())
        })
    })
    .await
}

#[tauri::command]
async fn series_add_archives(
    state: State<'_, AppState>,
    series_id: i64,
    archive_ids: Vec<i64>,
) -> Result<usize, String> {
    run_blocking(state, move |state| {
        with_library(state, |library| {
            series::add_archives(&library.conn, series_id, &archive_ids)
        })
    })
    .await
}

#[tauri::command]
async fn series_remove_archive(
    state: State<'_, AppState>,
    series_id: i64,
    archive_id: i64,
) -> Result<(), String> {
    run_blocking(state, move |state| {
        with_library(state, |library| {
            series::remove_archive(&library.conn, series_id, archive_id)
        })
    })
    .await
}

/// 取消已经排好的发布（还没退出时可用）。
#[tauri::command]
fn cancel_release(app: AppHandle) -> Result<(), String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|error| format!("无法获取配置目录: {error}"))?
        .join("release");
    let _ = std::fs::remove_file(dir.join("pending.json"));
    Ok(())
}

#[tauri::command]
async fn query_archives(
    app: AppHandle,
    state: State<'_, AppState>,
    query: ArchiveQuery,
) -> Result<ArchivePage, String> {
    run_blocking(state, move |state| {
        with_library(state, |library| {
            let covers_dir = paths::covers_dir(&app, &library.library_id)?;
            library::query_archives(&library.conn, &library.root, &covers_dir, &query)
        })
    })
    .await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {    // 单实例守卫必须在建窗口之前抢：抢不到说明已有实例在跑，
    // 这时去唤醒它并直接退出，避免第二个进程占住 WebView2 用户数据目录、
    // 也避免关不干净导致 exe 被锁、装完新版还是旧界面。
    // 闪退黑匣子：release 是 panic=abort，任何 Rust panic 都会让进程秒死，
    // WER 不留记录、stderr 在 GUI 子系统下无处可去——2026-11-21 的画廊闪退
    // 就是这样完全无证据。默认钩子之外再落一份到崩溃日志，下次闪退直接看这里。
    let crash_log = std::env::var("APPDATA").ok().map(|root| {
        std::path::Path::new(&root)
            .join("com.archivegallery.desktop")
            .join("crash.log")
    });
    std::panic::set_hook(Box::new(move |info| {
        let message = format!(
            "[{:?}] PANIC on thread '{}': {info}\n",
            std::time::SystemTime::now(),
            std::thread::current().name().unwrap_or("<unnamed>"),
        );
        eprint!("{message}");
        if let Some(path) = &crash_log {
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                let _ = std::io::Write::write_all(&mut file, message.as_bytes());
            }
        }
    }));

    let guard = match instance::acquire() {
        Some(guard) => guard,
        None => {
            if instance::wake_existing() {
                eprintln!("已有实例在运行，已请求它显示窗口，本次启动退出");
            } else {
                eprintln!("已有实例在运行（未能定位窗口），本次启动退出");
            }
            return;
        }
    };

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .register_uri_scheme_protocol("gallery", asset_protocol::serve)
        .register_uri_scheme_protocol("loose", move |context, request| {
            crate::loose_images::serve(context.app_handle(), request)
        })
        .setup(move |app| {
            // 上次发布成功后补记基线：之后关窗不会再无脑抬版本
            release::settle_baseline_after_success(app.handle());
            let state = AppState::new(app.handle());
            app.manage(state);
            // 守卫交给 Tauri 托管，活到进程结束；提前 drop 会立刻放开互斥体。
            app.manage(guard);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![            get_library_state,
            set_window_decorations,
            list_series,
            create_series,
            update_series,
            dissolve_series,
            series_set_cover,
            series_reorder,
            series_set_label,
            series_add_archives,
            series_remove_archive,
            open_library,
            import_paths,
            stage_import_paths,
            d2::d2_start,
            d2::d2_cancel,
            d2::d2_open,
            d2::d2_back,
            d2::d2_forward,
            d2::d2_reload,
            userscripts::gm_list_scripts,
            userscripts::gm_add_script_from_path,
            userscripts::gm_add_script_from_url,
            userscripts::gm_remove_script,
            userscripts::gm_set_script_enabled,
            loose_images::loose_list_collections,
            loose_images::loose_create_collection,
            loose_images::loose_delete_collection,
            loose_images::loose_scan_collection,
            loose_images::loose_import_files,
            loose_images::loose_list_images,
            loose_images::loose_set_rating,
            loose_images::loose_set_favorite,
            loose_images::loose_remove_image,
            loose_images::loose_collect_page,
            loose_images::loose_extract_page,
            loose_images::loose_open_folder,
            d2::d2_close_window,
            d2::d2_current_url,
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
            add_tags_bulk,
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
            read_anim_meta,
            read_reader_thumbnail,
            set_reading_progress,
            add_reader_bookmark,
            remove_reader_bookmark,
            list_queues,
            fill_queue,
            delete_queue,
            add_to_queue,
            remove_from_queue,
            list_queue_archives,
            create_queue,
            add_many_to_queue,
            move_queue_archive,
            set_tag_weight,
            get_release_state,
            save_release_settings,
            schedule_release,
            quit_for_release,
            cancel_release
        ])
        .on_window_event(|window, event| {
            // 关闭窗口时，如果用户开着「退出后自动发布」，就在这里把脚本排好并拉起。
            // 放在窗口事件而不是 ExitRequested：后者在 app.exit() 时也会触发，
            // 容易和「面板里点更新」那条路径重复安排。
            //
            // 性能纪律（管理员 2026-11-19 反馈「窗口停几秒才消失」）：
            // 这段代码跑完，窗口才会真正消失。所以这里只允许做「读内存 + 写几个小文件」：
            //   1. 先把窗口藏掉（后面无论多慢，用户都已经看到关掉了）；
            //   2. 有没有更新读后台缓存（`release::cached_has_changes`），绝不现跑 git；
            //   3. 拉起脚本走 wscript 直出，不再用 explorer（shell 激活会卡好几秒）。
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() != "main" {
                    return;
                }
                // 接管关闭：不再依赖「窗口一关 Tauri 就退进程」这套默认行为。
                // 只要还有子窗（D2 浏览器）活着，默认行为就是进程不退、单实例锁
                // 不释放，下次点快捷方式只会把旧窗口唤起来——用户看到的还是旧版。
                // 这里显式接管，保证「关主窗 = 整个画匣退出」。
                api.prevent_close();
                let started = std::time::Instant::now();
                let app = window.app_handle().clone();
                let settings = release::load_settings(&app);
                let should_release = settings.auto_on_exit && settings.source_dir.is_some();

                // 先把窗口藏掉：之后无论排发布、退进程多慢，用户都已经看到关掉了。
                let _ = window.hide();
                release::log_close_timing(&app, "窗口已隐藏", started.elapsed());

                // 排发布 + 退进程都放后台线程：git / 起脚本不卡事件循环。
                let handle = app.clone();
                std::thread::spawn(move || {
                    if should_release {
                        match release::cached_has_changes() {
                            Some(true) => release::schedule_release_now(&handle, &settings),
                            Some(false) => {}
                            // 缓存还没算出来（刚启动就关）：退回现算，此时窗口已藏，卡也看不见。
                            None => release::run_exit_release(&handle, &settings),
                        }
                        // 给脚本一点启动时间，然后再断开进程。
                        std::thread::sleep(std::time::Duration::from_millis(400));
                    }
                    // 彻底退出：exit 会关掉 D2 浏览器等所有子窗，进程真正结束、
                    // 单实例锁随之释放，下次启动才是新装的版本。
                    let _ = handle.exit(0);
                });
            }
            // 从编辑器切回来（大概刚提交完代码）：补算一次「有没有更新」缓存，
            // 免得 10 秒轮询还没跑到、关窗时用到旧结果。
            if let tauri::WindowEvent::Focused(true) = event {
                if window.label() == "main" {
                    release::refresh_changes_cache_in_background(window.app_handle().clone());
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    // 后台每 10 秒算一次「源码有没有更新」，关窗时只读缓存，不再现跑 git。
    release::spawn_changes_watcher(app.handle().clone());
    app.run(|_app, _event| {});
}

#[cfg(test)]
mod tests;
