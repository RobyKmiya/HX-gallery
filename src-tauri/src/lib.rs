mod anim;
mod archive;
mod asset_protocol;
mod backup;
mod browser;
mod blocked_elements;
mod container;
mod d2;
mod diag;
mod gallery_dl;
mod netdl;
pub mod db;
pub mod fsutil;
pub mod importer;
pub mod instance;
mod jm_api;
mod jm_decode;
pub mod library;
pub mod loose_images;
pub mod page_ops;
pub mod models;
mod pagecache;
mod paths;
pub mod queue;
mod reader;
mod release;
pub mod series;
pub mod site_login;
pub mod translate;
pub mod userscripts;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use tauri_plugin_global_shortcut::GlobalShortcutExt;
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
    /// 壁纸锁配置（管理员 2026-11-25）。旧配置文件没有这段时按 None 处理。
    #[serde(default)]
    wallpaper_lock: Option<WallpaperLock>,
}

struct AppInner {
    library: Option<db::LibraryContext>,
    last_library_path: Option<String>,
    startup_error: Option<String>,
    /// 库正在后台线程打开（外接硬盘唤醒/大库 SQLite 恢复）：
    /// 期间 get_library_state 报 loading=true，前端停在加载屏轮询。
    /// 2026-11-27 起 setup **不再同步开库**——主线程被硬盘唤醒卡住时，
    /// 连启动屏的 WebView 都初始化不了（管理员截图：黑屏无加载动效）。
    library_opening: bool,
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
            // 有上次的库路径就标记「后台打开中」；真正的验证与 SQLite 打开
            // 在 setup 拉起的后台线程里做，绝不碰主线程（外接硬盘唤醒
            // 会把整个启动流程连启动屏一起卡死，管理员 2026-11-27 二次反馈）。
            library_opening: settings.library_path.is_some(),
        };

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

/// 等维护锁（并行下载同时结束时的排队入口）。
///
/// `try_maintenance` 是立即失败：两本下载同时完成，后到的那本直接报
/// 「已有导入正在进行」，用户看到的就是「导入失败还得重新下」。
/// 这里改成排队等（下载 worker 线程多等一会儿完全无妨），默认最多 3 分钟。
fn wait_maintenance(timeout: std::time::Duration) -> Result<MutexGuard<'static, ()>, String> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match MAINTENANCE.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(_) if std::time::Instant::now() >= deadline => {
                return Err("已有导入或扫描长时间未完成，请稍后再试".to_string());
            }
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(400)),
        }
    }
}

pub(crate) fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
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

// ---------- 壁纸锁（管理员 2026-11-25 16:41） ----------
//
// 定位：防君子不防小人。不是安全锁，不做防破解。核心是「打开先看到自己的壁纸，
// 输密码才进库」。
//
// 救急：安装目录放 `关闭壁纸锁.cmd`，它只创建一个 `wallpaper-lock-off.flag`
// 标记文件；启动读配置时看到这个标记就把锁视为关闭，绕开密码。不碰 JSON、
// 不调 PowerShell，最不容易坏。要重新开启：设置 → 壁纸锁 关一次再开，
// 开启时会删掉这个标记。

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WallpaperSource {
    /// loose（散图）| archive（画廊页）| file（本地文件）
    #[serde(default)]
    pub kind: String,
    /// loose: 图片 id；archive: "archiveId:pageIndex"；file: 绝对路径
    #[serde(default)]
    pub reference: String,
    /// 显示区域（object-position 百分比，0-100）。锁屏壁纸**不再支持拖动平移**
    /// （管理员 2026-11-27：不好），显示区域改在设置页里拖这张图来定。
    #[serde(default)]
    pub crop: Option<WallpaperCrop>,
}

/// 壁纸显示区域：x/y 都是 0-100 的百分比（50/50 = 居中）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WallpaperCrop {
    #[serde(default)]
    pub x: f64,
    #[serde(default)]
    pub y: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WallpaperLock {
    #[serde(default)]
    pub enabled: bool,
    /// FNV-1a 64 + 盐。空串 = 无密码，点一下即进（默认无密码）。
    #[serde(default)]
    pub password_hash: String,
    #[serde(default)]
    pub salt: String,
    #[serde(default)]
    pub walls: Vec<WallpaperSource>,
    /// 轮换间隔秒；0 或只有一张时不轮换。
    #[serde(default)]
    pub interval_secs: u64,
    /// 是否已设密码（只给前端做界面分支用，哈希与盐不下发）。
    #[serde(default)]
    pub has_password: bool,
}

/// FNV-1a 64：够「防君子」，不引第三方依赖。
fn hash_password(password: &str, salt: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in salt.as_bytes().iter().chain(password.as_bytes()) {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn wallpaper_lock_flag_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("wallpaper-lock-off.flag")))
}

/// 救急脚本：只写一个标记文件，不需要 PowerShell / 改 JSON，最不容易出错。
fn ensure_lock_rescue_script() {
    let Some(flag) = wallpaper_lock_flag_path() else {
        return;
    };
    let Some(dir) = flag.parent() else {
        return;
    };
    let script = dir.join("关闭壁纸锁.cmd");
    if script.exists() {
        return;
    }
    let body = "@echo off\r\n\
> \"%~dp0wallpaper-lock-off.flag\" echo disabled\r\n\
echo 已关闭壁纸锁，下次启动画匣会直接进入。\r\n\
echo 想重新开启：打开画匣 - 设置 - 壁纸锁，关掉再开一次即可。\r\n\
pause\r\n";
    let _ = std::fs::write(&script, body.as_bytes());
}

#[tauri::command]
/// 壁纸本地缓存目录（AppData/wallpaper-cache）。壁纸锁源文件多半在外接
/// 硬盘上，唤醒时锁屏/启动屏跟着卡（管理员 2026-11-27 验收①）：保存配置时
/// 把图复制一份到本地，启动优先读缓存——外接硬盘睡不睡都不影响开屏。
fn wallpaper_cache_dir(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_config_dir()
        .ok()
        .map(|dir| dir.join("wallpaper-cache"))
}

/// 缓存文件名：kind + 引用串的 md5 前缀 + 原扩展名（取不到/可疑就 .jpg）。
fn wallpaper_cache_name(kind: &str, reference: &str) -> String {
    let digest = {
        use md5::{Digest, Md5};
        let mut hasher = Md5::new();
        hasher.update(format!("{kind}:{reference}").as_bytes());
        hasher.finalize()
    };
    let ext = Path::new(reference)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .filter(|e| !e.is_empty() && e.len() <= 5 && e.chars().all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or_else(|| "jpg".to_string());
    format!(
        "wp-{kind}-{}.{ext}",
        digest.iter().map(|b| format!("{b:02x}")).collect::<String>()
    )
}

/// 把一张壁纸的源文件复制进本地缓存（已有就跳过）。后台线程调用，错误忽略：
/// 缓存只是加速，失败时前端仍走原地址。
fn ensure_wallpaper_cached(app: &AppHandle, wall: &WallpaperSource) {
    let Some(dir) = wallpaper_cache_dir(app) else {
        return;
    };
    let name = wallpaper_cache_name(&wall.kind, &wall.reference);
    let dest = dir.join(&name);
    if dest.exists() {
        return;
    }
    let source_path: Option<PathBuf> = match wall.kind.as_str() {
        "file" => Some(PathBuf::from(&wall.reference)),
        "loose" => {
            // 散图：查库拿真实路径（库没开/记录缺失就放弃，下次保存时再试）。
            let Some(state) = app.try_state::<AppState>() else {
                return;
            };
            let Some((root, db_path)) = library_root_and_db(&state) else {
                return;
            };
            let Ok(id) = wall.reference.parse::<i64>() else {
                return;
            };
            crate::loose_images::fetch_record(&db_path, id).and_then(|record| {
                match record.kind.as_str() {
                    "file" => record.path.map(PathBuf::from),
                    _ => record
                        .rel_path
                        .map(|rel| root.join(rel))
                        .filter(|p| p.exists()),
                }
            })
        }
        // archive 来源走画廊协议（有页缓存），不复制。
        _ => return,
    };
    let Some(src) = source_path else {
        return;
    };
    if !src.exists() {
        return;
    }
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::copy(&src, &dest);
}

/// 后台把整组壁纸缓存到本地（不阻塞保存/读取）。
fn ensure_walls_cached(app: &AppHandle, walls: &[WallpaperSource]) {
    let app = app.clone();
    let walls = walls.to_vec();
    std::thread::spawn(move || {
        for wall in &walls {
            ensure_wallpaper_cached(&app, wall);
        }
    });
}

/// 逐张返回本地缓存文件名（null = 没缓存，前端退回原地址）。
#[tauri::command]
fn wallpaper_cache_urls(
    app: AppHandle,
    walls: Vec<WallpaperSource>,
) -> Vec<Option<String>> {
    // 顺手补缓存（第一次启动时缓存可能还没来得及生成）。
    ensure_walls_cached(&app, &walls);
    walls
        .iter()
        .map(|wall| {
            let dir = wallpaper_cache_dir(&app)?;
            let path = dir.join(wallpaper_cache_name(&wall.kind, &wall.reference));
            if path.exists() {
                path.file_name().map(|n| n.to_string_lossy().to_string())
            } else {
                None
            }
        })
        .collect()
}

/// 按文件头判断图片类型（返回 MIME）。认不出就返回 None——扩展名不可信。
fn sniff_image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        return Some("image/png");
    }
    if bytes.starts_with(b"GIF8") {
        return Some("image/gif");
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"BM") {
        return Some("image/bmp");
    }
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        return Some("image/avif");
    }
    None
}

/// `wcache://` 协议服务：只从壁纸缓存目录读文件（文件名白名单校验，防穿越）。
fn serve_wallpaper_cache(app: &AppHandle,
    request: tauri::http::Request<Vec<u8>>,
) -> tauri::http::Response<Vec<u8>> {
    use tauri::http::StatusCode;
    let not_found = |code: StatusCode, message: &str| {
        tauri::http::Response::builder()
            .status(code)
            .header("Content-Type", "text/plain; charset=utf-8")
            .body(message.as_bytes().to_vec())
            .unwrap_or_else(|_| tauri::http::Response::new(Vec::new()))
    };
    let name = request.uri().path().trim_start_matches('/').to_string();
    // 白名单：wp- 前缀 + 字母数字/点/横线/下划线，不允许斜杠。
    let valid = name.len() < 128
        && name.starts_with("wp-")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_');
    if !valid {
        return not_found(StatusCode::BAD_REQUEST, "缓存文件名无效");
    }
    let Some(dir) = wallpaper_cache_dir(app) else {
        return not_found(StatusCode::NOT_FOUND, "缓存目录不可用");
    };
    let path = dir.join(&name);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(_) => return not_found(StatusCode::NOT_FOUND, "缓存缺失"),
    };
    // 按**文件头**判类型，不看扩展名：缓存名是从「来源引用」算的（散图 id 没有
    // 扩展名 → 一律 .jpg），可散图实际多半是 webp。扩展名对不上时 Content-Type
    // 也会错，Chromium 有个别情况下就不解码了 → 锁屏/启动屏一片黑。
    let mime = match sniff_image_mime(&bytes) {
        Some(mime) => mime,
        None => return not_found(StatusCode::UNSUPPORTED_MEDIA_TYPE, "缓存不是图片"),
    };
    tauri::http::Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", mime)
        .body(bytes)
        .unwrap_or_else(|_| tauri::http::Response::new(Vec::new()))
}

/// 诊断与健康自检（1.2.4 提案 #1）：设置 → 诊断 面板调用。
#[tauri::command]
fn health_check(app: AppHandle) -> Vec<diag::HealthItem> {
    diag::run_health(&app)
}

/// 备份与迁移（1.2.5 提案 M2）。
#[tauri::command]
fn export_backup(app: AppHandle) -> Result<backup::BackupReport, String> {
    backup::export_backup(&app)
}

#[tauri::command]
fn list_backups(app: AppHandle) -> Vec<backup::BackupEntry> {
    backup::list_backups(&app)
}

#[tauri::command]
fn restore_backup(app: AppHandle, zip_path: String) -> Result<backup::RestoreReport, String> {
    backup::restore_backup(&app, &zip_path)
}

/// 导出诊断包（zip：自检结果 + d2 日志 + 脱敏设置），返回落盘路径。
#[tauri::command]
fn export_diag_bundle(app: AppHandle) -> Result<String, String> {
    diag::export_bundle(&app)
}

#[tauri::command]
fn get_wallpaper_lock(app: AppHandle) -> WallpaperLock {
    let settings = read_settings(&app);
    let mut lock = settings.wallpaper_lock.unwrap_or_default();
    if wallpaper_lock_flag_path()
        .map(|path| path.exists())
        .unwrap_or(false)
    {
        lock.enabled = false;
    }
    // 哈希与盐不下发前端；只告诉它「有没有密码」（先算再清，顺序不能反）。
    lock.has_password = !lock.password_hash.is_empty();
    lock.password_hash.clear();
    lock.salt.clear();
    lock
}

#[tauri::command]
fn save_wallpaper_lock(app: AppHandle, config: WallpaperLock) -> Result<WallpaperLock, String> {
    let mut settings = read_settings(&app);
    let existing = settings.wallpaper_lock.clone().unwrap_or_default();
    let next = WallpaperLock {
        enabled: config.enabled,
        // 密码只走 set_wallpaper_password，这里原样保留。
        has_password: !existing.password_hash.is_empty(),
        password_hash: existing.password_hash,
        salt: existing.salt,
        walls: config.walls,
        interval_secs: config.interval_secs,
    };
    if next.enabled {
        // 开启锁 = 删掉救急标记：用户明确要锁，就不再绕过。
        if let Some(flag) = wallpaper_lock_flag_path() {
            let _ = std::fs::remove_file(flag);
        }
        ensure_lock_rescue_script();
    }
    settings.wallpaper_lock = Some(next.clone());
    write_settings(&app, &settings);
    // 保存即缓存：把壁纸复制到本地，启动屏/锁屏不再依赖外接硬盘唤醒。
    ensure_walls_cached(&app, &next.walls);
    let mut out = next;
    out.password_hash.clear();
    out.salt.clear();
    Ok(out)
}

#[tauri::command]
fn set_wallpaper_password(app: AppHandle, password: String) -> Result<WallpaperLock, String> {
    let mut settings = read_settings(&app);
    let mut lock = settings.wallpaper_lock.clone().unwrap_or_default();
    if password.is_empty() {
        lock.password_hash.clear();
        lock.salt.clear();
    } else {
        let salt = format!(
            "{:x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        );
        lock.password_hash = hash_password(&password, &salt);
        lock.salt = salt;
    }
    lock.has_password = !lock.password_hash.is_empty();
    settings.wallpaper_lock = Some(lock.clone());
    write_settings(&app, &settings);
    let mut out = lock;
    out.password_hash.clear();
    out.salt.clear();
    Ok(out)
}

#[tauri::command]
fn verify_wallpaper_password(app: AppHandle, password: String) -> bool {
    let settings = read_settings(&app);
    let lock = settings.wallpaper_lock.unwrap_or_default();
    // 没有密码时，任何输入（包括空）都算通过——「默认无密码」。
    if lock.password_hash.is_empty() {
        return true;
    }
    hash_password(&password, &lock.salt) == lock.password_hash
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
            loading: false,
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
            loading: inner.library_opening,
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

        // 读-改-写：只更新 library_path，别把壁纸锁等其它设置一起冲掉。
        let mut settings = read_settings(&app);
        settings.library_path = Some(display_path);
        write_settings(&app, &settings);

        // 每日 settings 快照（1.2.5 M2：settings.json 不再是「丢了就没了」）。
        backup::daily_settings_snapshot(&app, &target);

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
            let (cover_key, rel_path) = db::purge_archive(&library.conn, archive_id)?;
            if let Some(key) = cover_key {
                // 先确认没有其它记录（含回收站）还在用这个封面，再删；
                // 同时校验 key 形态，避免被篡改的 cover_key 拼出缓存目录之外的路径。
                if archive::is_cover_key(&key) && !db::cover_key_in_use(&library.conn, &key)? {
                    let _ = fs::remove_file(covers_dir.join(&key));
                }
            }
            // 彻底删除 = 物理文件也移出库根。只删记录的话，下次扫描会按 rel_path 当成
            // 新文件重新导入（「删了又回来」）。移到 .gallery/trash/，扫描本来就跳过 .gallery。
            let source = library.root.join(&rel_path);
            if source.is_file() {
                let trash_dir = library.root.join(".gallery").join("trash");
                let _ = fs::create_dir_all(&trash_dir);
                let file_name = source
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| "archive".to_string());
                let dest = trash_dir.join(format!("{archive_id}-{file_name}"));
                if fs::rename(&source, &dest).is_err() {
                    // 跨卷或被占用：退回复制再删。失败也不拦「删除」本身，只是文件还在原位。
                    if fs::copy(&source, &dest).is_ok() {
                        let _ = fs::remove_file(&source);
                    }
                }
            }
            Ok(())
        })
    })
    .await
}

/// webview 导航时记一条历史。**必须异步**：on_navigation 跑在主线程，
/// 同步抢库锁会和别的线程互等（browser.rs 注释里的死锁铁律）。
pub(crate) fn record_browser_history_async(app: &AppHandle, url: String) {
    let app = app.clone();
    std::thread::spawn(move || {
        let Some(state) = app.try_state::<AppState>() else {
            return;
        };
        let inner = state.inner().clone();
        let Ok(guard) = inner.lock() else {
            return;
        };
        if let Some(library) = guard.library.as_ref() {
            let _ = db::record_history(&library.conn, &url, "");
        }
    });
}

// ---------- 网页元素屏蔽 ----------
#[tauri::command]
fn blocked_list(app: AppHandle) -> Vec<blocked_elements::BlockedEntry> {
    blocked_elements::list_flat(&app)
}

#[tauri::command]
fn blocked_remove(app: AppHandle, host: String, selector: String) -> Result<(), String> {
    blocked_elements::remove(&app, &host, &selector)
}

#[tauri::command]
fn blocked_clear(app: AppHandle) -> Result<(), String> {
    blocked_elements::clear(&app)
}

// ---------- 广告拦截：自定义名单读写 ----------
#[tauri::command]
fn adblock_read_custom() -> Result<String, String> {
    match browser::custom_adblock_path() {
        Some(path) => Ok(std::fs::read_to_string(path).unwrap_or_default()),
        None => Ok(String::new()),
    }
}

#[tauri::command]
fn adblock_write_custom(content: String) -> Result<(), String> {
    let path = browser::custom_adblock_path()
        .ok_or_else(|| "找不到 LOCALAPPDATA，无法写入拦截名单".to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {e}"))?;
    }
    std::fs::write(&path, content).map_err(|e| format!("写入拦截名单失败: {e}"))?;
    // 立即刷新缓存：改完就生效，不用重启。
    browser::reload_custom_adblock();
    Ok(())
}

// ---------- 浏览器账号密码：Windows DPAPI ----------
// 只绑当前 Windows 用户能解，落库是密文，不写明文、不进日志。
#[cfg(windows)]
fn dpapi_protect(data: &[u8]) -> Result<Vec<u8>, String> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };
    unsafe {
        let input = CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        let ok = CryptProtectData(
            &input,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        );
        if ok == 0 {
            return Err("DPAPI 加密失败".to_string());
        }
        let bytes = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(output.pbData as *mut core::ffi::c_void);
        Ok(bytes)
    }
}

#[cfg(windows)]
fn dpapi_unprotect(data: &[u8]) -> Result<Vec<u8>, String> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };
    unsafe {
        let input = CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        let ok = CryptUnprotectData(
            &input,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        );
        if ok == 0 {
            return Err("DPAPI 解密失败".to_string());
        }
        let bytes = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(output.pbData as *mut core::ffi::c_void);
        Ok(bytes)
    }
}

#[cfg(not(windows))]
fn dpapi_protect(data: &[u8]) -> Result<Vec<u8>, String> {
    Ok(data.to_vec())
}

#[cfg(not(windows))]
fn dpapi_unprotect(data: &[u8]) -> Result<Vec<u8>, String> {
    Ok(data.to_vec())
}

/// 从当前还活着的内嵌浏览器 / 主窗收集某个 host 的 Cookie，拼成请求头形式。
/// 只做读取，不写盘；写盘由 site_login_capture 负责。
fn collect_browser_cookies(app: &AppHandle, host: &str) -> String {
    let Ok(url) = tauri::Url::parse(&format!("https://{host}/")) else {
        return String::new();
    };
    let mut pairs: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (_label, webview) in app.webviews() {
        if let Ok(list) = webview.cookies_for_url(url.clone()) {
            for cookie in list {
                if seen.insert(cookie.name().to_string()) {
                    pairs.push(format!("{}={}", cookie.name(), cookie.value()));
                }
            }
        }
    }
    if let Some(window) = app.get_webview_window("main") {
        if let Ok(list) = window.cookies_for_url(url) {
            for cookie in list {
                if seen.insert(cookie.name().to_string()) {
                    pairs.push(format!("{}={}", cookie.name(), cookie.value()));
                }
            }
        }
    }
    pairs.join("; ")
}

/// 供 d2_set_cookies 复用：把一段 Cookie 串加密存成站点登录态快照。
pub(crate) fn save_site_login_snapshot(
    app: &AppHandle,
    host: &str,
    cookies: &str,
    note: &str,
) -> Result<(), String> {
    let state = app
        .try_state::<AppState>()
        .ok_or_else(|| "应用状态还没准备好".to_string())?;
    let enc = dpapi_protect(cookies.as_bytes())?;
    with_library(&state, |library| {
        db::upsert_site_login(&library.conn, host, &enc, note).map(|_| ())
    })
}

/// 给下载链路用的登录态兜底：从落盘快照里读该站 Cookie。
/// 浏览窗口已关、甚至画匣重启过也能拿到——这正是「每次下载都要重登」的解法。
pub(crate) fn stored_cookie_for_host(app: &AppHandle, host: &str) -> Option<String> {
    let state = app.try_state::<AppState>()?;
    let inner = state.lock().ok()?;
    let library = inner.library.as_ref()?;
    let enc = db::site_login_cookies(&library.conn, host).ok()??;
    let plain = dpapi_unprotect(&enc).ok()?;
    let text = String::from_utf8_lossy(&plain).to_string();
    if text.is_empty() { None } else { Some(text) }
}

/// 给浏览器登录框自动填充用：按 host 从账号簿取一组明文库名 / 密码。
pub(crate) fn credential_for_host(app: &AppHandle, host: &str) -> Option<(String, String)> {
    let state = app.try_state::<AppState>()?;
    let inner = state.lock().ok()?;
    let library = inner.library.as_ref()?;
    let (_, username, enc) = db::credential_for_host(&library.conn, host).ok()??;
    let plain = dpapi_unprotect(&enc).ok()?;
    let password = String::from_utf8_lossy(&plain).to_string();
    if username.is_empty() || password.is_empty() {
        return None;
    }
    Some((username, password))
}

/// 抓住当前浏览器里某个站点的登录态，加密落盘。
/// 用户操作路径：内置浏览器登录成功 → 设置→浏览器数据→登录态→保存当前站点。
#[tauri::command]
async fn site_login_capture(
    app: AppHandle,
    state: State<'_, AppState>,
    host: String,
    note: Option<String>,
) -> Result<usize, String> {
    let host = db::normalize_host(&host);
    if host.is_empty() {
        return Err("请填写站点域名".to_string());
    }
    let cookies = crate::site_login::capture_from_tabs(&app, &host);
    if cookies.is_empty() {
        return Err("没读到该站点的登录态：请先在网页标签里打开并登录这个站点".to_string());
    }
    let count = cookies.split(';').filter(|part| part.contains('=')).count();
    let enc = dpapi_protect(cookies.as_bytes())?;
    let note = note.unwrap_or_default();
    run_blocking(state, move |state| {
        with_library(state, |library| {
            db::upsert_site_login(&library.conn, &host, &enc, &note)
        })
    })
    .await?;
    crate::netdl::clear_cookie_cache();
    Ok(count)
}

/// 手动粘贴 Cookie 入库：不需要浏览器开着也能生效。
///
/// 管理员 2026-11-26：「登录过、关掉画匣、再打开数据又没了」——以前的粘贴框
/// 只会往「已打开的浏览窗口」里写，没开窗就整条丢失。现在先落加密库，
/// 下发给下载链路和已打开的标签。
#[tauri::command]
async fn site_login_import(
    app: AppHandle,
    host: String,
    cookies: String,
) -> Result<usize, String> {
    crate::site_login::import_cookie_text(&app, &host, &cookies)
}

/// 18comic 账号密码直接登录（移动端 API）。
///
/// 内嵌浏览器里 18comic 的网页登录按钮点了没反应：它依赖 Google reCAPTCHA，
/// gstatic 加载不出来就永远拿不到 token（管理员 2026-11-27 反馈登录不了）。
/// 移动接口登录不碰网页：POST /login 换回 AVS cookie，走既有的
/// 「存快照 + 注入已开标签 + 刷新」链路（import_cookie_text）。
#[tauri::command]
async fn jm_api_login(
    app: AppHandle,
    state: State<'_, AppState>,
    username: String,
    password: String,
) -> Result<usize, String> {
    if username.trim().is_empty() || password.is_empty() {
        return Err("账号和密码都要填".to_string());
    }
    let avs = crate::jm_api::api_login(&app, username.trim(), &password)?;
    // 顺带把账密存进浏览器账号簿（DPAPI）：登录框自动填充（credential_for_host）
    // 从此有 18comic 的账号可填，不再「密码栏空的还不提示」。
    let site = "18comic.vip".to_string();
    let user = username.trim().to_string();
    let note = "API 登录自动保存".to_string();
    run_blocking(state, move |state| {
        with_library(state, |library| {
            let enc = dpapi_protect(password.as_bytes())?;
            db::upsert_credential(&library.conn, None, &site, &user, Some(&enc), &note)
        })
    })
    .await
    .map_err(|e| format!("登录成功，但保存账号失败：{e}"))?;
    crate::site_login::import_cookie_text(&app, "18comic.vip", &format!("AVS={avs}"))
}

/// 看某条登录态快照里存了哪些 cookie 的**名字**（值不回显）。
///
/// 「注入了还是没登录」多半是快照里根本没有登录标志 cookie（比如 18comic 的
/// AVS 过期后被浏览器的自动保存覆盖成了一堆杂项），让用户能亲眼看到差什么。
#[tauri::command]
async fn site_login_detail(
    state: State<'_, AppState>,
    id: i64,
) -> Result<Vec<String>, String> {
    run_blocking(state, move |state| {
        with_library(state, |library| {
            let enc = db::site_login_cookies_by_id(&library.conn, id)?
                .ok_or_else(|| "这条登录态不存在".to_string())?;
            let plain = dpapi_unprotect(&enc)?;
            let text = String::from_utf8_lossy(&plain).to_string();
            let mut names: Vec<String> = Vec::new();
            for part in text.split([';', '\n', '\r']) {
                let piece = part.trim();
                if let Some((name, value)) = piece.split_once('=') {
                    let name = name.trim().to_string();
                    if !name.is_empty() && !value.trim().is_empty() && !names.contains(&name) {
                        names.push(name);
                    }
                }
            }
            Ok(names)
        })
    })
    .await
}

/// 用画匣专属配置的**真 Edge** 打开一个站点（--app 无边框窗）。
///
/// 这是「接入浏览器」路线的试水口：cookie、Cloudflare、扩展全归 Edge 自己管，
/// 登录一次永久有效，不再依赖我们的快照/回填。画匣只负责启动；
/// 二期再接 CDP 把 cookie/下载接管回来。
#[tauri::command]
fn open_in_edge(app: AppHandle, url: String) -> Result<(), String> {
    let parsed = tauri::Url::parse(&url).map_err(|e| format!("网址不对：{e}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("只支持 http/https 网址".to_string());
    }
    const CANDIDATES: &[&str] = &[
        "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
        "C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe",
    ];
    let exe = CANDIDATES
        .iter()
        .find(|path| std::path::Path::new(path).exists())
        .ok_or("没找到 Microsoft Edge，请先确认系统里有 Edge")?;
    // 独立 profile：和你日常用的 Edge 数据完全隔离，画匣专属。
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("拿配置目录失败：{e}"))?
        .join("edge-profile");
    let _ = std::fs::create_dir_all(&dir);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new(exe)
            .creation_flags(0x0800_0000) // 不弹控制台
            .arg(format!("--user-data-dir={}", dir.display()))
            .arg("--no-first-run")
            .arg(format!("--app={parsed}"))
            .spawn()
            .map_err(|e| format!("启动 Edge 失败：{e}"))?;
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new(exe)
            .arg(format!("--user-data-dir={}", dir.display()))
            .arg("--no-first-run")
            .arg(format!("--app={parsed}"))
            .spawn()
            .map_err(|e| format!("启动 Edge 失败：{e}"))?;
    }
    Ok(())
}

#[tauri::command]
async fn site_login_list(
    state: State<'_, AppState>,
) -> Result<Vec<db::SiteLoginEntry>, String> {
    run_blocking(state, move |state| {
        with_library(state, |library| db::list_site_logins(&library.conn))
    })
    .await
}

#[tauri::command]
async fn site_login_delete(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    run_blocking(state, move |state| {
        with_library(state, |library| db::delete_site_login(&library.conn, id))
    })
    .await
}

#[tauri::command]
async fn browser_history_list(
    state: State<'_, AppState>,
    search: Option<String>,
    limit: Option<i64>,
) -> Result<Vec<db::HistoryEntry>, String> {
    run_blocking(state, move |state| {
        with_library(state, |library| {
            db::list_history(&library.conn, search.as_deref(), limit.unwrap_or(200))
        })
    })
    .await
}

#[tauri::command]
async fn browser_history_delete(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    run_blocking(state, move |state| {
        with_library(state, |library| db::delete_history(&library.conn, id))
    })
    .await
}

#[tauri::command]
async fn browser_history_clear(state: State<'_, AppState>) -> Result<(), String> {
    run_blocking(state, move |state| {
        with_library(state, |library| db::clear_history(&library.conn))
    })
    .await
}

#[tauri::command]
async fn browser_credential_list(
    state: State<'_, AppState>,
) -> Result<Vec<db::CredentialEntry>, String> {
    run_blocking(state, move |state| {
        with_library(state, |library| db::list_credentials(&library.conn))
    })
    .await
}

#[tauri::command]
async fn browser_credential_save(
    state: State<'_, AppState>,
    id: Option<i64>,
    site: String,
    username: String,
    password: Option<String>,
    note: Option<String>,
) -> Result<i64, String> {
    run_blocking(state, move |state| {
        with_library(state, |library| {
            // 空密码 = 不改；非空 = DPAPI 加密后落库。
            let enc = match password.as_deref() {
                Some(value) if !value.is_empty() => Some(dpapi_protect(value.as_bytes())?),
                _ => None,
            };
            db::upsert_credential(
                &library.conn,
                id,
                site.trim(),
                username.trim(),
                enc.as_deref(),
                note.as_deref().unwrap_or(""),
            )
        })
    })
    .await
}

#[tauri::command]
async fn browser_credential_delete(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    run_blocking(state, move |state| {
        with_library(state, |library| db::delete_credential(&library.conn, id))
    })
    .await
}

#[tauri::command]
async fn browser_credential_reveal(state: State<'_, AppState>, id: i64) -> Result<String, String> {
    run_blocking(state, move |state| {
        with_library(state, |library| {
            let enc = db::credential_password(&library.conn, id)?;
            let plain = dpapi_unprotect(&enc)?;
            Ok(String::from_utf8_lossy(&plain).to_string())
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
async fn gallery_page_list(
    state: State<'_, AppState>,
    id: i64,
) -> Result<crate::page_ops::PageList, String> {
    run_blocking(state, move |state| {
        with_library(state, |library| {
            crate::page_ops::page_list(&library.conn, &library.root, id)
        })
    })
    .await
}

/// 删除第 index 页（0 基）：重打包 zip/cbz，被删页进 .gallery/trash/pages/。
#[tauri::command]
async fn gallery_page_delete(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    index: i64,
) -> Result<i64, String> {
    let app = app.clone();
    run_blocking(state, move |state| {
        with_library(state, |library| {
            crate::page_ops::page_delete(&app, &library.conn, &library.root, id, index)
        })
    })
    .await
}

/// 追加图片：原生选择器挑图，追加到包尾。返回（新增页数，总页数）。
#[tauri::command]
async fn gallery_page_add(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> Result<(usize, i64), String> {
    let app = app.clone();
    run_blocking(state, move |state| {
        with_library(state, |library| {
            crate::page_ops::page_add(&app, &library.conn, &library.root, id)
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

/// 更新收藏夹的侧栏图标（形状 + 填充色）。
#[tauri::command]
async fn update_collection_style(
    state: State<'_, AppState>,
    collection_id: i64,
    icon: String,
    color: String,
) -> Result<(), String> {
    run_blocking(state, move |state| {
        with_library(state, |library| {
            db::set_collection_style(&library.conn, collection_id, &icon, &color)
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
        let password = with_library(&state, |library| {
            Ok(db::archive_password(&library.conn, archive_id))
        })
        .unwrap_or(None);
        reader::read_page_with_password(&path, index, pin, password.as_deref())
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
        let password = with_library(&state, |library| {
            Ok(db::archive_password(&library.conn, archive_id))
        })
        .unwrap_or(None);
        reader::read_thumbnail_with_password(&path, index, max_edge, password.as_deref())
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

/// 把阅读器当前页解码成 RGBA 写进系统剪贴板（管理员 11-22 右键菜单需求）。
/// 页面在压缩包里，先取字节再解码；动图取第一帧。
#[tauri::command]
fn copy_reader_page(
    app: tauri::AppHandle,
    archive_id: i64,
    page_index: i64,
) -> Result<(), String> {
    use tauri_plugin_clipboard_manager::ClipboardExt;

    let path = reader::remembered_archive(archive_id)
        .ok_or_else(|| "找不到画集文件（刚启动的话先翻一页再试）".to_string())?;
    let password = with_library(&app.state::<AppState>(), |library| {
        Ok(db::archive_password(&library.conn, archive_id))
    })
    .unwrap_or(None);
    let (_name, bytes) =
        reader::read_page_bytes_with_password(&path, page_index, password.as_deref())?;
    let img = image::load_from_memory(&bytes).map_err(|e| format!("解码当前页失败: {e}"))?;
    let rgba = img.to_rgba8();
    let clipboard_image = tauri::image::Image::new(rgba.as_raw(), rgba.width(), rgba.height());
    app.clipboard()
        .write_image(&clipboard_image)
        .map_err(|e| format!("写入剪贴板失败: {e}"))
}

/// 在资源管理器里定位画集文件（选中它）。
#[tauri::command]
fn reveal_in_explorer(path: String) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    if !std::path::Path::new(&path).exists() {
        return Err("画集文件不存在（可能已被移动或删除）".to_string());
    }
    // explorer 的 /select, 参数逗号后直接跟路径，整段当一个 raw arg 传，
    // 带空格的路径用引号包住；explorer 退出码 1 是常态，不当作失败。
    std::process::Command::new("explorer")
        .raw_arg(format!("/select,\"{}\"", path))
        .creation_flags(0x0800_0000)
        .spawn()
        .map_err(|e| format!("打开资源管理器失败: {e}"))?;
    Ok(())
}

/// 当前页导出成 PNG 并设为桌面壁纸（SPI_SETDESKWALLPAPER，fWinIni=3 立即生效）。
/// 页面在压缩包里，先解出 PNG 落到库目录，再把路径交给系统。
#[tauri::command]
fn set_desktop_wallpaper(
    app: tauri::AppHandle,
    archive_id: i64,
    page_index: i64,
) -> Result<(), String> {
    use std::os::windows::process::CommandExt;

    let path = reader::remembered_archive(archive_id)
        .ok_or_else(|| "找不到画集文件（刚启动的话先翻一页再试）".to_string())?;
    let password = with_library(&app.state::<AppState>(), |library| {
        Ok(db::archive_password(&library.conn, archive_id))
    })
    .unwrap_or(None);
    let (_name, bytes) =
        reader::read_page_bytes_with_password(&path, page_index, password.as_deref())?;
    let img = image::load_from_memory(&bytes).map_err(|e| format!("解码当前页失败: {e}"))?;

    // PNG 落到库目录的 .gallery 下（和封面缓存一个屋檐，不污染画廊）。
    let library_root = with_library(&app.state::<AppState>(), |library| {
        Ok(library.root.clone())
    })?;
    let wallpaper_dir = library_root.join(".gallery").join("wallpaper");
    std::fs::create_dir_all(&wallpaper_dir).map_err(|e| format!("建壁纸目录失败: {e}"))?;
    let png_path = wallpaper_dir.join("wallpaper.png");
    img.save_with_format(&png_path, image::ImageFormat::Png)
        .map_err(|e| format!("导出 PNG 失败: {e}"))?;

    let script = format!(
        "Add-Type -MemberDefinition '[DllImport(\"user32.dll\")] public static extern int SystemParametersInfo(int uAction, int uParam, string lpParam, int fWinIni);' -Name W -Namespace N; [N.W]::SystemParametersInfo(20, 0, '{}', 3)",
        png_path.to_string_lossy().replace('\'', "''")
    );
    let status = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .creation_flags(0x0800_0000)
        .status()
        .map_err(|e| format!("启动系统脚本失败: {e}"))?;
    if !status.success() {
        return Err("设置壁纸失败（系统脚本非零退出）".to_string());
    }
    Ok(())
}

/// 老板键第一步：把主窗整个藏起来（恢复走全局热键，默认 Ctrl+Shift+H，可在设置里改）。
#[tauri::command]
fn hide_main_window(app: tauri::AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "主窗口不存在".to_string())?;
    window.hide().map_err(|e| format!("隐藏窗口失败: {e}"))
}

/// 前端首帧就绪：亮出主窗、关掉原生启动屏（splash.html）。
/// 启动屏解决的是「外接硬盘唤醒阻塞 setup + WebView2 冷启动」期间
/// 窗口一片空白毫无反馈的问题（管理员 2026-11-27 二次反馈）。
#[tauri::command]
fn splash_finish(app: tauri::AppHandle) {
    if let Some(splash) = app.get_webview_window("splash") {
        let _ = splash.close();
    }
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.show();
        let _ = main.unminimize();
        let _ = main.set_focus();
    }
}

/// M5③：把一个工作台标签拆到独立窗口。
/// 标签数据由主窗写进 localStorage（token 索引），新窗加载 index.html 时
/// 从 ?detach=<token> 读回——避免中文标题/URL 经 query 编码失真。
/// 新窗关闭只关自己：on_window_event 里只对 main 接管 exit。
#[tauri::command]
fn detach_tab(app: tauri::AppHandle, token: String) -> Result<String, String> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};
    let label = format!("hx-detached-{}", uuid::Uuid::new_v4().simple());
    let built = WebviewWindowBuilder::new(
        &app,
        &label,
        WebviewUrl::App(format!("index.html?detach={}", token).into()),
    )
    .title("画匣 · 独立标签")
    .inner_size(1100.0, 760.0)
    .min_inner_size(480.0, 360.0)
    .build();
    match built {
        Ok(window) => {
            let _ = window.set_focus();
            Ok(label)
        }
        Err(error) => Err(format!("拆出独立窗口失败：{error}")),
    }
}

/// 老板键呼回键的持久化路径（config/shortcuts.json）。
fn boss_recall_path(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    app.path()
        .app_config_dir()
        .ok()
        .map(|dir| dir.join("shortcuts.json"))
}

/// 读当前呼回键绑定；没有存过或读坏了就给默认 Ctrl+Shift+H。
fn load_boss_recall(app: &tauri::AppHandle) -> String {
    boss_recall_path(app)
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|value| value["bossRecall"].as_str().map(str::to_string))
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "Ctrl+Shift+H".to_string())
}

/// 注册全局呼回键（注册失败返回 Err，调用方决定怎么提示）。
fn register_boss_recall(app: &tauri::AppHandle, binding: &str) -> Result<(), String> {
    let sc = binding.trim().to_lowercase();
    if sc.is_empty() {
        return Err("呼回键不能为空".to_string());
    }
    // 先摘掉旧的（失败忽略——可能本来就没注册上），再注册新的。
    let _ = app.global_shortcut().unregister(load_boss_recall(app).to_lowercase().as_str());
    app.global_shortcut()
        .register(sc.as_str())
        .map_err(|e| format!("注册全局热键失败: {e}"))
}

/// 呼回键当前绑定（设置面板展示用）。
#[tauri::command]
fn get_boss_recall(app: tauri::AppHandle) -> String {
    load_boss_recall(&app)
}

/// 改呼回键：注册成功才落盘，失败不破坏现有绑定。
#[tauri::command]
fn set_boss_recall(app: tauri::AppHandle, binding: String) -> Result<(), String> {
    register_boss_recall(&app, &binding)?;
    if let Some(path) = boss_recall_path(&app) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(
            &path,
            serde_json::json!({ "bossRecall": binding.trim() }).to_string(),
        );
    }
    Ok(())
}

/// 窗口几何节流落盘（管理员 11-22：重启保持窗口大小/位置）。
/// Moved/Resized 一秒能触发上百次，直接写盘会卡 UI；用原子门 + 延迟线程，
/// 900ms 内只有第一笔真正排上，落的是停稳之后的最新几何。
static GEOMETRY_SAVE_PENDING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// 直接读当前窗口几何写盘（**物理像素**，与读取侧保持一致）。
///
/// 关闭时会同步调它：防抖 900ms，如果用户拖完窗就关，最后一笔还没来得及落盘，
/// 下次启动就回到老位置——这就是「记不住上次位置」的直接原因。
fn write_window_geometry(window: &tauri::Window) -> bool {
    let Ok(config_dir) = window.app_handle().path().app_config_dir() else {
        return false;
    };
    let _ = std::fs::create_dir_all(&config_dir);
    // 最小化时 Windows 把窗口挪到 (-32000,-32000) 并缩成 0x0，这些垃圾几何
    // 一旦落盘，下次启动就是离屏小窗（1.0.67 事故）。一律不写。
    if window.is_minimized().unwrap_or(false) {
        return false;
    }
    let maximized = window.is_maximized().unwrap_or(false);
    let mut value = serde_json::json!({ "maximized": maximized });
    if !maximized {
        let (Ok(position), Ok(size)) = (window.outer_position(), window.inner_size()) else {
            return false;
        };
        if position.x < -10000 || position.y < -10000 || size.width < 400 || size.height < 300 {
            return false;
        }
        value["x"] = position.x.into();
        value["y"] = position.y.into();
        value["width"] = size.width.into();
        value["height"] = size.height.into();
    }
    std::fs::write(config_dir.join("window.json"), value.to_string()).is_ok()
}

fn persist_window_geometry_debounced(app: tauri::AppHandle) {
    use std::sync::atomic::Ordering;
    if GEOMETRY_SAVE_PENDING.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(900));
        GEOMETRY_SAVE_PENDING.store(false, Ordering::SeqCst);
        if let Some(window) = app.get_window("main") {
            write_window_geometry(&window);
        }
    });
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
        .plugin(tauri_plugin_clipboard_manager::init())
        // 老板键呼回：Ctrl+Shift+H 全局切换主窗显隐。窗口藏起来之后应用内
        // 按键收不到，必须用系统级热键才能唤回。
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state == tauri_plugin_global_shortcut::ShortcutState::Pressed {
                        if let Some(window) = app.get_webview_window("main") {
                            let visible = window.is_visible().unwrap_or(false);
                            if visible {
                                let _ = window.hide();
                            } else {
                                let _ = window.show();
                                let _ = window.unminimize();
                                let _ = window.set_focus();
                            }
                        }
                    }
                })
                .build(),
        )
        .register_uri_scheme_protocol("gallery", asset_protocol::serve)
        .register_uri_scheme_protocol("loose", move |context, request| {
            crate::loose_images::serve(context.app_handle(), request)
        })
        .register_uri_scheme_protocol("wcache", |context, request| {
            serve_wallpaper_cache(context.app_handle(), request)
        })
        .setup(move |app| {
            // **manage 必须是 setup 闭包的第一件事**。Tauri 2 的窗口创建发生在
            // setup 闭包之前（tauri::app::setup(): 先 for window_config build()
            // 再跑用户闭包），前端首屏的 getLibraryState 很可能抢在 manage 之前
            // 到达 IPC → 必报「state not managed」（管理员三轮反馈的启动报错根因）。
            // 以前 manage 排在闭包末尾（老板键/几何恢复之后），竞态窗口更大。
            {
                let state = AppState::new(app.handle());
                app.manage(state);
            }
            // 兜底定时器必须先于一切可能阻塞的活儿启动：外接硬盘在休眠时，
            // 下面 AppState::new → db::open_library 会把 setup 整个卡住几十秒。
            // 前端首帧就绪会调 splash_finish 正常交接；这条定时器防的是
            // 「前端永远没起来」（WebView 崩了/资源缺失）的极端情况。
            {
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(30));
                    if let Some(splash) = handle.get_webview_window("splash") {
                        let _ = splash.close();
                    }
                    if let Some(main) = handle.get_webview_window("main") {
                        let _ = main.show();
                        let _ = main.set_focus();
                    }
                });
            }
            // 上次发布成功后补记基线：之后关窗不会再无脑抬版本
            release::settle_baseline_after_success(app.handle());
            // 内置「画匣盾」用户脚本：首次启动写入脚本仓库，之后不覆盖（用户可停用）。
            userscripts::ensure_builtin_scripts(app.handle());
            // 注册老板键呼回（默认 Ctrl+Shift+H，可在设置里改；失败不拦启动，只记日志）。
            {
                let binding = load_boss_recall(app.handle());
                if let Err(error) = register_boss_recall(app.handle(), &binding) {
                    release::log_close_timing(app.handle(), &format!("注册老板键失败: {error}"), std::time::Duration::ZERO);
                }
            }
            // 恢复上次窗口几何（管理员 11-22：窗口大小要灵活，重启保持）
            if let Some(window) = app.get_webview_window("main") {
                if let Ok(config_dir) = app.path().app_config_dir() {
                    if let Ok(text) = std::fs::read_to_string(config_dir.join("window.json")) {
                        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                            if value["maximized"].as_bool().unwrap_or(false) {
                                let _ = window.maximize();
                            } else if let (Some(w), Some(h)) =
                                (value["width"].as_f64(), value["height"].as_f64())
                            {
                                // 恢复也要过安检：离屏坐标 / 零尺寸的垃圾几何直接丢弃，
                                // 保留 Tauri 默认（居中 1280x820）。
                                let on_screen = value["x"].as_f64().map_or(true, |x| {
                                    x > -10000.0 && x < 20000.0
                                }) && value["y"].as_f64().map_or(true, |y| {
                                    y > -10000.0 && y < 20000.0
                                });
                                if w >= 400.0 && h >= 300.0 && on_screen {
                                    // 落盘的是**物理像素**（见 write_window_geometry），这里必须用
                                    // Physical 还原：以前用 Logical，150% 缩放下尺寸和坐标被放大 1.5 倍，
                                    // 于是「一开就特别大、位置也记不对」。
                                    // 再按当前显示器 clamp，换屏/改缩放后也不会超出屏幕。
                                    let monitor = window.current_monitor().ok().flatten();
                                    let (max_w, max_h, mon_x, mon_y) = monitor
                                        .map(|monitor| {
                                            let size = monitor.size();
                                            let position = monitor.position();
                                            (
                                                size.width as f64,
                                                size.height as f64,
                                                position.x as f64,
                                                position.y as f64,
                                            )
                                        })
                                        .unwrap_or((1280.0, 820.0, 0.0, 0.0));
                                    let width = w.min(max_w * 0.95).max(640.0);
                                    let height = h.min(max_h * 0.95).max(460.0);
                                    let _ = window.set_size(tauri::PhysicalSize::new(
                                        width.round() as u32,
                                        height.round() as u32,
                                    ));
                                    if let (Some(x), Some(y)) =
                                        (value["x"].as_f64(), value["y"].as_f64())
                                    {
                                        // 窗口必须**整体**落在屏幕工作区里：上限 = 屏幕右下 - 窗口尺寸。
                                        // 以前只限制「起点别太靠下」，size 大时下半截仍在屏外，
                                        // 表现为窗口跑到屏幕最底下、只剩一条黑边拖不动（1.1.54 事故）。
                                        let max_cx = (mon_x + max_w - width).max(mon_x);
                                        let max_cy = (mon_y + max_h - height).max(mon_y);
                                        let cx = x.clamp(mon_x, max_cx);
                                        let cy = y.clamp(mon_y, max_cy);
                                        let _ = window.set_position(tauri::PhysicalPosition::new(
                                            cx.round() as i32,
                                            cy.round() as i32,
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
            }
            // AppState 已在闭包顶部 manage（见上）；这里只补单实例守卫——
            // 守卫交给 Tauri 托管，活到进程结束；提前 drop 会立刻放开互斥体。
            app.manage(guard);
            // 后台开库：验证目录 + SQLite 打开放到独立线程。外接硬盘在休眠时，
            // 唤醒动辄十几秒——以前同步做，主线程被卡住，连启动屏的 WebView
            // 都初始化不了（管理员看到的「黑屏无加载动效」）。现在启动屏、
            // 锁屏、主窗全部先跑起来，数据源在后台慢慢醒，前端轮询 getLibraryState。
            {
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    let Some(state) = handle.try_state::<AppState>() else {
                        return;
                    };
                    let last_path = state
                        .lock()
                        .ok()
                        .and_then(|inner| inner.last_library_path.clone());
                    let result = match last_path {
                        None => Ok(None),
                        Some(path) => {
                            let library_path = PathBuf::from(&path);
                            let initialized = library_path.is_dir()
                                && library_path.join(".gallery").join("library.db").exists();
                            if !initialized {
                                Err("上次的库目录不存在或尚未初始化".to_string())
                            } else {
                                match db::open_library(&library_path) {
                                    Ok(library) => {
                                        eprintln!(
                                            "opened library at {}（后台）",
                                            library.root.to_string_lossy()
                                        );
                                        backup::daily_settings_snapshot(&handle, &library.root);
                                        Ok(Some(library))
                                    }
                                    Err(error) => {
                                        eprintln!("failed to open library: {error}");
                                        Err(format!("上次的库无法打开：{error}"))
                                    }
                                }
                            }
                        }
                    };
                    if let Ok(mut inner) = state.lock() {
                        inner.library_opening = false;
                        match result {
                            Ok(Some(library)) => {
                                inner.library = Some(library);
                                inner.startup_error = None;
                            }
                            Ok(None) => {}
                            Err(error) => inner.startup_error = Some(error),
                        }
                    }
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![            get_library_state,
            health_check,
            export_diag_bundle,
            export_backup,
            list_backups,
            restore_backup,
            set_window_decorations,
            hide_main_window,
            splash_finish,
            detach_tab,
            get_boss_recall,
            set_boss_recall,
            copy_reader_page,
            reveal_in_explorer,
            set_desktop_wallpaper,
            get_wallpaper_lock,
            save_wallpaper_lock,
            wallpaper_cache_urls,
            set_wallpaper_password,
            verify_wallpaper_password,
            browser_history_list,
            browser_history_delete,
            browser_history_clear,
            browser_credential_list,
            browser_credential_save,
            browser_credential_delete,
            browser_credential_reveal,
            site_login_capture,
            site_login_list,
            site_login_delete,
            site_login_import,
            site_login_detail,
            jm_api_login,
            open_in_edge,
            adblock_read_custom,
            adblock_write_custom,
            blocked_list,
            blocked_remove,
            blocked_clear,
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
            d2::d2_embed_open,
            d2::d2_embed_sync,
            d2::d2_embed_show,
            d2::d2_embed_close,
            d2::browser_settings_get,
            d2::browser_settings_set,
            d2::d2_back,
            d2::d2_forward,
            d2::d2_reload,
            d2::d2_set_cookies,
            d2::d2_collect_tags,
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
            gallery_page_list,
            gallery_page_delete,
            gallery_page_add,
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
            update_collection_style,
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
            cancel_release,
            gallery_dl::dl_open_panel,
            gallery_dl::dl_scan,
            gallery_dl::dl_start,
            gallery_dl::dl_status,
            gallery_dl::dl_cancel,
            gallery_dl::dl_pick_folder,
            gallery_dl::dl_open_folder,
            netdl::net_dl_preview,
            netdl::net_dl_start,
            netdl::net_dl_start_task,
            netdl::net_dl_status,
            netdl::net_dl_status_all,
            netdl::net_dl_cancel,
            netdl::net_dl_cancel_task,
            netdl::net_dl_pause_task,
            netdl::net_dl_remove_task
        ])
        .on_window_event(|window, event| {
            if matches!(
                event,
                tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_)
            ) && window.label() == "main" {
                persist_window_geometry_debounced(window.app_handle().clone());
            }
            // 关窗 = 直接退出。原「退出后自动发布」（auto_on_exit）已按管理员
            // 2026-11-22 要求撤掉：发版只由施工 agent 手动跑 release.mjs——
            // 自动发布与手动发版抢 cargo 构建锁 + 版本号竞争，一天内坑了两次。
            //
            // 性能纪律不变：这段代码跑完窗口才会真正消失，只允许
            // 「读内存 + 写几个小文件」，先藏窗再退进程。
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() != "main" {
                    return;
                }
                // 接管关闭：只要还有子窗（D2 浏览器）活着，默认行为就是进程不退、
                // 单实例锁不释放，下次点快捷方式只会把旧窗口唤起来。
                // 这里显式接管，保证「关主窗 = 整个画匣退出」。
                // 关窗瞬间同步落一次几何：防抖还没到点就被关掉的话，
                // 位置/大小就白记了（用户反馈「记不住上次位置」）。
                write_window_geometry(window);
                api.prevent_close();
                let _ = window.hide();
                let handle = window.app_handle().clone();
                std::thread::spawn(move || {
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
