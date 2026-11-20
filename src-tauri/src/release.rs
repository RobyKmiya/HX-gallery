//! 面板内的「发布更新」入口。
//!
//! 约束：真正的发布要 `npm run release`（Node + cargo + 源码目录），而且它必须
//! **在本进程退出之后**才能跑——脚本会结束 `archive-gallery.exe` 并覆盖它自己。
//! 所以这里只做三件事：
//!   1. 校验用户登记的源码目录（必须含 `scripts/release.mjs`）；
//!   2. 生成一个 `release-on-exit.mjs`（node 编排脚本），由 wscript+node 拉起，
//!      让它的父进程是资源管理器而不是本应用，本进程被杀也不影响它；
//!   3. 立刻返回，前端随即退出应用，脚本接手：等进程消失 → 跑 release → 重新拉起。
//!
//! 状态与日志落在 `%APPDATA%\com.archivegallery.desktop\release\`，
//! 下次启动可以读到上次结果。

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

/// 更新任务的登记状态。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ReleaseSettings {
    /// 源码目录（含 `package.json` 与 `scripts/release.mjs`）。
    pub source_dir: Option<String>,
    /// 关闭应用后是否自动执行发布。
    pub auto_on_exit: bool,
    /// 发布完成后是否自动重新启动画匣。
    pub relaunch_after: bool,
}

/// 一次发布任务的结果，供面板回显。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ReleaseStatus {
    /// 是否已经安排好一次发布（等待退出）。
    pub pending: bool,
    /// 上一次发布的结果：success / failed / running / none。
    pub last_result: String,
    /// 上一次发布的时间（毫秒时间戳）。
    pub last_at: i64,
    /// 上一次发布的摘要或失败原因（脚本写回）。
    pub last_message: String,
    /// 脚本路径，方便用户自己看一眼。
    pub script_path: String,
    /// 日志路径。
    pub log_path: String,
}

fn release_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|error| format!("无法获取配置目录: {error}"))?;
    let dir = config_dir.join("release");
    fs::create_dir_all(&dir).map_err(|error| format!("无法创建发布目录: {error}"))?;
    Ok(dir)
}

fn settings_file(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(release_dir(app)?.join("release.json"))
}

fn status_file(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(release_dir(app)?.join("status.json"))
}

fn pending_file(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(release_dir(app)?.join("pending.json"))
}

fn published_file(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(release_dir(app)?.join("published.json"))
}

/// 上次成功发布时的源码基线。由 release.mjs 在发布成功后写入。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct PublishedState {
    head: String,
    version: String,
    at: i64,
}

fn read_json<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> T {
    fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let raw = serde_json::to_string_pretty(value)
        .map_err(|error| format!("序列化失败: {error}"))?;
    let temp = path.with_extension("json.tmp");
    fs::write(&temp, raw).map_err(|error| format!("写入失败: {error}"))?;
    fs::rename(&temp, path).map_err(|error| format!("替换失败: {error}"))
}

pub fn load_settings(app: &AppHandle) -> ReleaseSettings {
    settings_file(app)
        .map(|path| read_json(&path))
        .unwrap_or_default()
}

pub fn load_status(app: &AppHandle) -> ReleaseStatus {
    let mut status: ReleaseStatus = status_file(app)
        .map(|path| read_json(&path))
        .unwrap_or_default();
    if status.last_result.is_empty() {
        status.last_result = "none".to_string();
    }
    if let Ok(dir) = release_dir(app) {
        status.script_path = dir.join("release-on-exit.mjs").to_string_lossy().to_string();
        status.log_path = dir.join("release.log").to_string_lossy().to_string();
        status.pending = dir.join("pending.json").exists();
    }
    status
}

/// 校验并规范化源码目录：必须同时有 `package.json` 和 `scripts/release.mjs`。
pub fn validate_source_dir(raw: &str) -> Result<PathBuf, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("源码目录不能为空".to_string());
    }
    let candidate = PathBuf::from(trimmed);
    if !candidate.is_absolute() {
        return Err("源码目录必须是绝对路径".to_string());
    }
    if !candidate.is_dir() {
        return Err(format!("目录不存在：{}", candidate.to_string_lossy()));
    }
    if !candidate.join("package.json").is_file() {
        return Err("该目录里没有 package.json，可能不是画匣源码目录".to_string());
    }
    if !candidate.join("scripts").join("release.mjs").is_file() {
        return Err("该目录里没有 scripts/release.mjs，无法执行发布".to_string());
    }
    Ok(candidate)
}

pub fn save_settings(app: &AppHandle, settings: &ReleaseSettings) -> Result<(), String> {
    let path = settings_file(app)?;
    write_json(&path, settings)
}

/// 发布完成后记录基线，供下次关窗判断是否值得再发一次。
/// 由 Rust 侧在确认发布成功时调用（比 Node 侧写更可靠，路径统一在这里）。
pub fn record_published(app: &AppHandle, source_dir: &Path, version: &str) {
    let head = git_output(source_dir, &["rev-parse", "HEAD"]).unwrap_or_default();
    let state = PublishedState {
        head: head.trim().to_string(),
        version: version.to_string(),
        at: crate::fsutil::now_ms(),
    };
    if let Ok(path) = published_file(app) {
        let _ = write_json(&path, &state);
    }
}

fn git_output(dir: &Path, args: &[&str]) -> Result<String, String> {
    let mut command = std::process::Command::new("git");
    command.args(args).current_dir(dir);
    // 本程序是 GUI 子系统进程，不带 CREATE_NO_WINDOW 时每次拉起 git 都会
    // 闪一个黑色控制台窗（「关窗自动发布」每 10 秒轮询一次，肉眼可见）。
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let output = command
        .output()
        .map_err(|e| format!("git 调用失败: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// 判断源码相对上次发布是否有「实质更新」。
///
/// 规则（管理员 2026-11-19 明确要求：关一次不等于更新一次）：
/// - 从未成功发布过 → 视为有更新，允许首次发布（建立基线）；
/// - 当前 HEAD 与上次发布记录的 HEAD 不同 → 有新提交 → 有更新；
/// - HEAD 相同但工作区脏 → 有未提交的源码改动 → 有更新；
/// - 其余 → 无实质更新，关窗应安静退出、不抬版本、不打包。
///
/// 保守原则：git 不可用 / 读不到基线时返回 true（宁可多发一次，不可漏发），
/// 并且所有判断都只读，绝不修改仓库状态。
pub fn has_source_changes(app: &AppHandle, settings: &ReleaseSettings) -> bool {
    let Some(raw) = settings.source_dir.as_deref() else {
        return false;
    };
    let Ok(source) = validate_source_dir(raw) else {
        return false;
    };

    let baseline: PublishedState = published_file(app)
        .map(|p| read_json(&p))
        .unwrap_or_default();

    let head = git_output(&source, &["rev-parse", "HEAD"])
        .ok()
        .map(|s| s.trim().to_string());
    // 注意：只脏「版本号文件」不算改动。发布脚本自己抬版本就会让工作区变脏，
    // 若照旧算作「有更新」，关窗 → 发布 → 抬版本 → 又脏 → 再发布，会无限自激。
    let dirty = git_output(&source, &["status", "--porcelain"])
        .ok()
        .map(|s| has_real_changes(&s));

    decide_has_changes(&baseline.head, head.as_deref(), dirty)
}

/// 抬版本会改到的文件：它们的改动是发布流程自己造的，不该再触发一轮发布。
const VERSION_FILES: [&str; 4] = [
    "package.json",
    "src-tauri/Cargo.lock",
    "src-tauri/Cargo.toml",
    "src-tauri/tauri.conf.json",
];

/// `git status --porcelain` 里是否存在「版本号文件以外」的改动。
///
/// 纯函数，便于单测。porcelain 每行形如 ` M path` / `?? path` / `R  old -> new`。
fn has_real_changes(porcelain: &str) -> bool {
    porcelain.lines().any(|line| {
        if line.trim().is_empty() {
            return false;
        }
        let raw = line.get(3..).unwrap_or("").trim();
        // 重命名行取箭头右边的新路径
        let path = raw.rsplit(" -> ").next().unwrap_or(raw).trim();
        // 含空格/中文的路径会被 git 加引号
        let path = path.trim_matches('"');
        !VERSION_FILES.contains(&path)
    })
}

/// 关窗耗时诊断：往 `release/close-timing.log` 追加一行。
///
/// 只用于定位「关窗卡顿」，写一行几字节，失败就算了，绝不能因此影响关闭。
pub fn log_close_timing(app: &AppHandle, stage: &str, elapsed: std::time::Duration) {
    use std::io::Write;
    let Ok(dir) = release_dir(app) else { return };
    let line = format!(
        "[{}] {:>18} {:>7.1}ms\n",
        crate::fsutil::now_ms(),
        stage,
        elapsed.as_secs_f64() * 1000.0
    );
    if let Ok(mut file) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("close-timing.log"))
    {
        let _ = file.write_all(line.as_bytes());
    }
}

/// 「源码有没有实质更新」的关窗缓存。
///
/// 关窗路径上不能现跑 git：一条 `rev-parse` + 一条 `status --porcelain`，再加上拉起脚本，
/// 会让窗口多留几百毫秒到几秒（管理员 2026-11-19 反馈「窗口停几秒才消失」）。
/// 改成后台线程定期算好，关窗只读一个原子变量。
///
/// 取值：0 = 还没算出来，1 = 无更新，2 = 有更新。
static CHANGES_CACHE: std::sync::atomic::AtomicI8 = std::sync::atomic::AtomicI8::new(0);
/// 上一次补算的时间戳（毫秒），避免焦点事件密集时狂起线程。
static LAST_REFRESH_MS: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

/// 关窗时读缓存；`None` 表示后台还没来得及算（极少数：刚启动就关）。
pub fn cached_has_changes() -> Option<bool> {
    use std::sync::atomic::Ordering;
    match CHANGES_CACHE.load(Ordering::Relaxed) {
        0 => None,
        1 => Some(false),
        _ => Some(true),
    }
}

/// 立刻算一次并写进缓存（只该在后台线程调用：内部会起 git 子进程）。
pub fn refresh_changes_cache(app: &AppHandle) {
    use std::sync::atomic::Ordering;
    let settings = load_settings(app);
    let value = if settings.auto_on_exit
        && settings.source_dir.is_some()
        && has_source_changes(app, &settings)
    {
        2
    } else {
        1
    };
    CHANGES_CACHE.store(value, Ordering::Relaxed);
}

/// 启动时挂上：每 10 秒刷新一次缓存，让关窗永远不用等 git。
///
/// 代价是关窗判断最多滞后 10 秒；从编辑器回来刚提交完代码时，主窗获得焦点还会补算一次。
pub fn spawn_changes_watcher(app: AppHandle) {
    std::thread::spawn(move || loop {
        refresh_changes_cache(&app);
        std::thread::sleep(std::time::Duration::from_secs(10));
    });
}

/// 补算一次（3 秒内只算一次），供窗口获得焦点时调用。
pub fn refresh_changes_cache_in_background(app: AppHandle) {
    use std::sync::atomic::Ordering;
    let now = crate::fsutil::now_ms();
    let last = LAST_REFRESH_MS.load(Ordering::Relaxed);
    if now.saturating_sub(last) < 3_000 {
        return;
    }
    LAST_REFRESH_MS.store(now, Ordering::Relaxed);
    std::thread::spawn(move || refresh_changes_cache(&app));
}

/// 关窗时把发布脚本排好并拉起（调用方已用缓存判断过，这里不再碰 git）。
pub fn schedule_release_now(app: &AppHandle, settings: &ReleaseSettings) {
    match schedule_of(app, settings) {
        Ok((script, _)) => match launch_detached(Path::new(&script)) {
            Ok(()) => eprintln!("已安排退出后自动发布"),
            Err(error) => eprintln!("安排退出后发布失败: {error}"),
        },
        Err(error) => eprintln!("安排退出后发布失败: {error}"),
    }
}

/// 发布失败后的冷却期：避免「编译失败 → 关窗 → 又编译」反复拖死机器。
const FAIL_COOLDOWN_MS: i64 = 30 * 60 * 1000;

/// 上一次发布失败且仍在冷却期内。
fn in_fail_cooldown(app: &AppHandle) -> bool {
    let Ok(path) = status_file(app) else {
        return false;
    };
    let status: ReleaseStatus = read_json(&path);
    if status.last_result != "failed" {
        return false;
    }
    crate::fsutil::now_ms().saturating_sub(status.last_at) < FAIL_COOLDOWN_MS
}

/// 退出前的收尾：判断源码有没有实质更新，有就把发布脚本排好并拉起。
///
/// 调用方保证**只能在后台线程调用**：这里会起 git 子进程、写脚本、用 explorer
/// 拉起隐藏启动器，全在主线程上做会让窗口消失得慢半拍
/// （管理员 2026-11-19 反馈「关窗卡好一会，不能秒关」）。
pub fn run_exit_release(app: &AppHandle, settings: &ReleaseSettings) {
    // 闸门一：已经排好一次发布就等它跑完，绝不再叠第二份。
    // 2026-11-20 管理员反馈「一直闪黑色窗口、开了马上关好几次」：没有这道闸门时，
    // 连按几次关闭就拉起多份发布编排，各自 npm/cargo/taskkill 互踩。
    if pending_file(app).map(|path| path.exists()).unwrap_or(false) {
        eprintln!("已有排队的发布任务，跳过本次关窗触发");
        return;
    }
    // 闸门二：上次发布刚失败过，冷却期内不再自动重试。
    if in_fail_cooldown(app) {
        eprintln!("上次发布失败且仍在冷却期，跳过本次关窗触发");
        return;
    }
    if !has_source_changes(app, settings) {
        eprintln!("源码无实质更新，正常退出（不发布、不抬版本）");
        return;
    }
    match schedule_of(app, settings) {
        Ok((script, _)) => match launch_detached(Path::new(&script)) {
            Ok(()) => eprintln!("已安排退出后自动发布"),
            Err(error) => eprintln!("安排退出后发布失败: {error}"),
        },
        Err(error) => eprintln!("安排退出后发布失败: {error}"),
    }
}

/// 纯判断逻辑（可单测）。git 读不到时用 None 表示，按保守原则当成「有更新」。
fn decide_has_changes(baseline_head: &str, head: Option<&str>, dirty: Option<bool>) -> bool {
    // 从未发布过：允许这次发布（并建立基线）
    if baseline_head.is_empty() {
        return true;
    }
    // git 不可用 → 保守，允许发布
    let Some(head) = head else { return true };
    if head != baseline_head {
        return true;
    }
    // HEAD 相同，看工作区；读不到也保守当成脏
    dirty.unwrap_or(true)
}

/// 应用启动时调用：上次发布成功过，就把「当前 HEAD + 当时版本」记为基线。
///
/// 时机选在启动而非发布脚本末尾，是因为发布完成那一刻 exe 正在被替换、
/// Node 侧拿到的 git 状态未必稳定；而用户下次打开画匣时，仓库 HEAD 正是刚发布的那次提交。
/// 只补记一次（published.json 的 version 与 status 的 lastResult 对应版本不同才写）。
pub fn settle_baseline_after_success(app: &AppHandle) {
    let status = load_status(app);
    if status.last_result != "success" || status.last_at <= 0 {
        return;
    }
    let Some(raw) = load_settings(app).source_dir else {
        return;
    };
    let Ok(source) = validate_source_dir(&raw) else {
        return;
    };
    let current: PublishedState = published_file(app)
        .map(|p| read_json(&p))
        .unwrap_or_default();
    // 已记录过这次发布（版本号一致）就不重复写，避免把用户后续的新提交误记为已发布
    if current.version == read_pkg_version(&source) && !current.head.is_empty() {
        return;
    }
    record_published(app, &source, &read_pkg_version(&source));
}

fn read_pkg_version(source: &Path) -> String {
    fs::read_to_string(source.join("package.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|v| v.get("version").and_then(|s| s.as_str()).map(str::to_string))
        .unwrap_or_default()
}

/// 供 UI 展示：把判断结果连同原因返回，让用户看得见「为什么不更新」。
pub fn release_readiness(app: &AppHandle, settings: &ReleaseSettings) -> serde_json::Value {
    let baseline: PublishedState = published_file(app)
        .map(|p| read_json(&p))
        .unwrap_or_default();
    let head = settings.source_dir.as_deref()
        .and_then(|raw| validate_source_dir(raw).ok())
        .and_then(|src| git_output(&src, &["rev-parse", "HEAD"]).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let dirty = settings.source_dir.as_deref()
        .and_then(|raw| validate_source_dir(raw).ok())
        .and_then(|src| git_output(&src, &["status", "--porcelain"]).ok())
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    let changed = has_source_changes(app, settings);
    serde_json::json!({
        "hasBaseline": !baseline.head.is_empty(),
        "lastVersion": baseline.version,
        "lastHead": baseline.head,
        "currentHead": head,
        "dirty": dirty,
        "hasChanges": changed,
        "reason": if !settings.auto_on_exit {
            "自动更新未开启"
        } else if baseline.head.is_empty() {
            "尚无发布基线，下次关闭将发布一次作为基线"
        } else if !head.is_empty() && head != baseline.head {
            "源码有新提交"
        } else if dirty {
            "源码有未提交改动"
        } else if changed {
            "检测到源码变化"
        } else {
            "自上次发布以来源码无变化，关窗不会更新"
        },
    })
}

/// rust 侧生成的 **node 编排脚本**（2026-11-20 重写，替代原来的
/// `script_body`(.cmd) + `helper_source`(helper.mjs) 双文件）：
///
/// 1. 旧形态是一个含 `taskkill` 裸文本的 `.cmd` 批处理——在本机被杀软
///    （火绒的行为检测）静默删除：脚本写出去几秒内就消失，vbs 指着不存在的
///    文件，关窗自动发布整条链静默失败（2026-11-20 管理员机器实锤两例，
///    取证存档 agent-dispatch/logs/vibe/release-on-exit-调查-20261120.md）。
///    改为 **wscript 直接调 node 跑本编排脚本**：链路里不再有 `.cmd` 文件；
///    结束进程只是最后一道保险，正常关窗流程应用几百毫秒内自己就退了。
/// 2. node 读 UTF-8 源码，中文路径/文案不需要任何编码垫付；
/// 3. 等待用 `tasklist`（只读查询），60 秒超时才 `taskkill /F` 兜底；
/// 4. 全程写 UTF-8 日志与 status.json，供面板与失败冷却判断。
fn orchestrator_source(
    exe: &Path,
    source_dir: &Path,
    log: &Path,
    status: &Path,
    pending: &Path,
    hud_script: &Path,
    relaunch: bool,
) -> String {
    let json = |value: &Path| -> String {
        serde_json::to_string(&value.to_string_lossy().to_string())
            .unwrap_or_else(|_| "\"\"".to_string())
    };
    format!(
        r#"// 由画匣生成，随一次更新任务存在。勿手改。
import fs from 'node:fs';
import {{ spawn, spawnSync }} from 'node:child_process';
import path from 'node:path';

const EXE = {exe};
const SOURCE = {source};
const LOG = {log};
const STATUS = {status};
const PENDING = {pending};
const HUD = {hud};
const RELAUNCH = {relaunch};

const LABEL = {{
  start: '开始',
  wait: '等待应用退出',
  timeout_kill: '等待超时，强制结束进程',
  exited: '应用已退出',
  build: '执行 npm run release',
  ok: '更新完成',
  fail: '更新失败，详见日志',
  done: '流程结束',
  skipped: '已有一条发布链在运行，本次不重复发布',
}};

function say(key) {{
  const text = LABEL[key] ?? key;
  const line = `[${{new Date().toISOString()}}] ${{text}}\r\n`;
  try {{
    fs.appendFileSync(LOG, line, 'utf8');
  }} catch {{
    // 日志写不进去也不能让流程中断
  }}
  console.log(text);
}}

function writeStatus(result, message) {{
  try {{
    fs.writeFileSync(
      STATUS,
      JSON.stringify({{ lastResult: result, lastAt: Date.now(), lastMessage: message }}),
      'utf8',
    );
  }} catch {{
    // 忽略
  }}
}}

function sleep(ms) {{
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
}}

function appRunning() {{
  const probe = spawnSync('tasklist', [
    '/FI', 'IMAGENAME eq ' + path.basename(EXE), '/NH',
  ], {{ windowsHide: true }});
  if (probe.error) return false; // 查不到进程当它已退，别把发布卡死
  return String(probe.stdout || '').includes(path.basename(EXE));
}}

// —— 单飞锁 ——
// 连关两窗会拉起两条发布链：两条 release 同时跑 cargo 抢 target 锁，
// 发布看起来像卡死（2026-11-21 凌晨 1.0.36/37 连环失败的根因之一）。
// 有活链时本次直接退出，pending 一并清掉，下次关窗再发。
const LOCK = path.join(path.dirname(LOG), 'release.lock');
try {{
  let alive = false;
  let prevPid = 0;
  if (fs.existsSync(LOCK)) {{
    prevPid = Number(fs.readFileSync(LOCK, 'utf8').trim());
    if (Number.isFinite(prevPid) && prevPid > 0 && prevPid !== process.pid) {{
      try {{
        process.kill(prevPid, 0);
        alive = true;
      }} catch (e) {{
        alive = Boolean(e && e.code === 'EPERM');
      }}
    }}
  }}
  const fresh = alive && Date.now() - fs.statSync(LOCK).mtimeMs < 2 * 60 * 60 * 1000;
  if (fresh) {{
    writeStatus('skipped', '已有一条发布链在运行（pid ' + prevPid + '），本次关窗不重复发布');
    say('skipped');
    try {{ fs.rmSync(PENDING, {{ force: true }}); }} catch {{ }}
    process.exit(0);
  }}
  fs.writeFileSync(LOCK, String(process.pid), 'utf8');
}} catch {{
  // 锁不可用就照旧跑：宁可偶尔双跑，也不能因为锁失败把发布堵死
}}
process.on('exit', () => {{
  try {{
    if (fs.readFileSync(LOCK, 'utf8').trim() === String(process.pid)) {{
      fs.rmSync(LOCK, {{ force: true }});
    }}
  }} catch {{
    // 忽略
  }}
}});

// —— 主流程 ——
say('start');
say('wait');
let tries = 0;
while (appRunning()) {{
  tries += 1;
  if (tries >= 60) {{
    say('timeout_kill');
    spawnSync('taskkill', ['/IM', path.basename(EXE), '/F'], {{ windowsHide: true }});
    sleep(2000);
    break;
  }}
  sleep(1000);
}}
say('exited');

// 悬浮进度窗（release.mjs 每阶段写 progress.json，HUD 读它画进度条）。
try {{
  const hud = spawn(
    'powershell.exe',
    ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-WindowStyle', 'Hidden', '-File', HUD],
    {{ detached: true, stdio: 'ignore' }},
  );
  hud.unref();
}} catch {{
  // HUD 挂了不影响发布
}}

say('build');
const child = spawn(
  'cmd.exe',
  ['/d', '/s', '/c', 'npm run release'],
  {{ cwd: SOURCE, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] }},
);
child.stdout.on('data', (chunk) => {{ try {{ fs.appendFileSync(LOG, chunk); }} catch {{}} }});
child.stderr.on('data', (chunk) => {{ try {{ fs.appendFileSync(LOG, chunk); }} catch {{}} }});
child.on('error', (error) => {{
  say('fail');
  writeStatus('failed', String(error));
  finish(1);
}});
child.on('close', (code) => {{
  if (code === 0) {{
    say('ok');
    writeStatus('success', 'release finished');
    finish(0);
  }} else {{
    say('fail');
    writeStatus('failed', 'npm run release failed with exit code ' + code);
    finish(1);
  }}
}});

function finish(code) {{
  // pending 标记是「已排队」的门闩：跑到这必然已收尾，无论成败都摘掉。
  let wanted = null;
  if (RELAUNCH && code === 0) {{
    try {{
      wanted = JSON.parse(fs.readFileSync(PENDING, 'utf8'));
    }} catch {{
      // 读不到就当不需要重启
    }}
  }}
  try {{ fs.rmSync(PENDING, {{ force: true }}); }} catch {{}}
  if (wanted?.relaunch && fs.existsSync(EXE)) {{
    try {{
      spawn(EXE, [], {{ detached: true, stdio: 'ignore', cwd: path.dirname(EXE) }}).unref();
    }} catch {{
      // 重启失败不影响发布结果
    }}
  }}
  say('done');
  process.exit(code);
}}
"#,
        exe = json(exe),
        source = json(source_dir),
        log = json(log),
        status = json(status),
        pending = json(pending),
        hud = json(hud_script),
        relaunch = relaunch,
    )
}
/// explorer 直接拉 `.cmd` 会带出一个可见控制台，发布期间一挂好几分钟。
/// 这里生成一个 VBS 垫片：wscript 是 GUI 进程本身不弹窗，再以窗口样式 0
/// （隐藏、不等待）启动 node 编排脚本，整条发布链路（含 `npm run release`）都无窗口。
/// VBS 必须写成 **UTF-16LE + BOM**：wscript 按 ANSI 读无 BOM 文件，中文路径会碎。
/// 这里直接 `node <脚本>`（CreateProcess 按 PATH 解析 node.exe，2026-11-20 实测
/// wscript 分离环境里 node 在 PATH）；链路里没有 .cmd，火绒不再有可 flagged 的
/// 批处理文本。
fn launcher_source(script: &Path) -> String {
    // VBS 的字符串转义是把 `"` 翻倍，不能用 JSON 转义（反斜杠会被写成 \\）。
    let path_vbs = script.to_string_lossy().replace('"', "\"\"");
    format!(
        "' Generated by archive-gallery. Do not edit.\r\n\
Set sh = CreateObject(\"WScript.Shell\")\r\n\
sh.Run \"node \" & Chr(34) & \"{path_vbs}\" & Chr(34), 0, False\r\n"
    )
}

/// 关窗更新期间的悬浮进度窗（PowerShell WinForms，DPI aware）。
///
/// 主程序退出后，发布脚本在后台跑约 20 分钟，没有任何可见反馈——用户没法
/// 判断「什么时候更新好」。release.mjs 在每个阶段写 `progress.json`（含各阶段
/// 预计时长），这个置顶小窗每 0.5 秒读一次：加权进度条 + 已用时长 + 预计剩余；
/// 可拖动（模拟标题栏 HTCAPTION）、可最小化成小条再还原；完成后自动关闭并给出
/// 「打开画匣」，失败则停留并显示错误。用 PowerShell WinForms 而不是 HTA：
/// mshta 窗口置不了顶（会被前台应用整个盖住，「悬浮」名存实亡），WinForms 有
/// 原生 TopMost。启动即 SetProcessDPIAware，控件尺寸乘缩放系数——高缩放屏
/// 文字不发虚、坐标按物理像素算。PS 这里按 5.1 兼容写法（无三元、无链式 ??），
/// 脚本由 `write_utf8_bom` 落盘（PS5.1 读无 BOM UTF-8 会按 ANSI 拆坏中文）。
pub(crate) fn progress_hud_source(progress_json: &str, installed_exe: &str) -> String {
    let ps_quote = |value: &str| format!("'{}'", value.replace('\'', "''"));
    let template = r##"# 由画匣生成，随一次更新任务存在。勿手改。
# 关窗更新悬浮进度窗：主程序已退出，后台发布约 20 分钟，置顶显示阶段进度。
# - 0.5 秒轮询 progress.json；
# - 可拖动（模拟标题栏 HTCAPTION）、可最小化成小条（再点还原）；
# - 进度按 release.mjs 提供的各阶段预计时长加权，本步内按已用时间推进，
#   并给出「预计剩约」。预计值按本机历史构建校准，快了 ETA 偏乐观，
#   慢了进度条会在 96% 前缓慢爬行并标注「本步比平时慢」。
# - 启动即声明 DPI aware：高缩放屏上文字不发虚，坐标全按物理像素算，
#   控件尺寸统一乘缩放系数 $s（96dpi 屏 $s=1，与设计稿一致）。
Add-Type -AssemblyName System.Windows.Forms | Out-Null
Add-Type -AssemblyName System.Drawing | Out-Null

# 无边框窗体拖动靠这个：把左键按下伪装成标题栏拖动（HTCAPTION）
Add-Type -TypeDefinition @"
namespace HudNative {
    using System;
    using System.Runtime.InteropServices;
    public static class Drag {
        [DllImport("user32.dll")]
        public static extern bool ReleaseCapture();
        [DllImport("user32.dll")]
        public static extern int SendMessage(IntPtr hWnd, int msg, IntPtr wParam, IntPtr lParam);
        [DllImport("user32.dll")]
        public static extern bool SetProcessDPIAware();
        [DllImport("user32.dll")]
        public static extern uint GetDpiForSystem();
    }
}
"@
[void][HudNative.Drag]::SetProcessDPIAware()
try { $script:s = [double][HudNative.Drag]::GetDpiForSystem() / 96.0 } catch { $script:s = 1.0 }
if ($script:s -le 0) { $script:s = 1.0 }

$progressPath = __PROGRESS_PATH__
$exePath = __EXE_PATH__

$form = New-Object System.Windows.Forms.Form
$form.Text = '画匣更新'
$form.FormBorderStyle = 'None'
$form.TopMost = $true
$form.StartPosition = 'Manual'
$form.Size = New-Object System.Drawing.Size(396, 172)
$form.BackColor = [System.Drawing.Color]::FromArgb(16, 18, 22)

$fg = [System.Drawing.Color]::FromArgb(232, 234, 240)
$muted = [System.Drawing.Color]::FromArgb(139, 147, 167)
$accent = [System.Drawing.Color]::FromArgb(124, 147, 255)
$danger = [System.Drawing.Color]::FromArgb(255, 107, 107)

$title = New-Object System.Windows.Forms.Label
$title.Bounds = New-Object System.Drawing.Rectangle(16, 12, 306, 22)
$title.ForeColor = $fg
$title.Font = New-Object System.Drawing.Font('Microsoft YaHei UI', 10, [System.Drawing.FontStyle]::Bold)
$title.BackColor = [System.Drawing.Color]::Transparent
$title.Text = '画匣 · 正在更新'
$form.Controls.Add($title)

$closeButton = New-Object System.Windows.Forms.Button
$closeButton.Bounds = New-Object System.Drawing.Rectangle(360, 8, 28, 24)
$closeButton.Text = [char]0x00d7
$closeButton.FlatStyle = 'Flat'
$closeButton.FlatAppearance.BorderColor = [System.Drawing.Color]::FromArgb(42, 45, 54)
$closeButton.ForeColor = $muted
$closeButton.Add_Click({ $form.Close() })
$form.Controls.Add($closeButton)

$minButton = New-Object System.Windows.Forms.Button
$minButton.Bounds = New-Object System.Drawing.Rectangle(328, 8, 28, 24)
$minButton.Text = [char]0x2013
$minButton.FlatStyle = 'Flat'
$minButton.FlatAppearance.BorderColor = [System.Drawing.Color]::FromArgb(42, 45, 54)
$minButton.ForeColor = $muted
$minButton.Add_Click({ Set-Collapsed $true })
$form.Controls.Add($minButton)

$barBack = New-Object System.Windows.Forms.Panel
$barBack.Bounds = New-Object System.Drawing.Rectangle(16, 46, 364, 8)
$barBack.BackColor = [System.Drawing.Color]::FromArgb(42, 45, 54)
$form.Controls.Add($barBack)

$bar = New-Object System.Windows.Forms.Panel
$bar.Bounds = New-Object System.Drawing.Rectangle(0, 0, 2, 8)
$bar.BackColor = $accent
$barBack.Controls.Add($bar)

$stage = New-Object System.Windows.Forms.Label
$stage.Bounds = New-Object System.Drawing.Rectangle(16, 62, 364, 20)
$stage.ForeColor = [System.Drawing.Color]::FromArgb(198, 203, 216)
$stage.BackColor = [System.Drawing.Color]::Transparent
$stage.Text = '正在启动更新…'
$form.Controls.Add($stage)

$meta = New-Object System.Windows.Forms.Label
$meta.Bounds = New-Object System.Drawing.Rectangle(16, 84, 364, 18)
$meta.ForeColor = $muted
$meta.BackColor = [System.Drawing.Color]::Transparent
$meta.Text = '已用 00:00'
$form.Controls.Add($meta)

$errorLabel = New-Object System.Windows.Forms.Label
$errorLabel.Bounds = New-Object System.Drawing.Rectangle(16, 104, 364, 30)
$errorLabel.ForeColor = [System.Drawing.Color]::FromArgb(255, 157, 157)
$errorLabel.BackColor = [System.Drawing.Color]::Transparent
$errorLabel.Visible = $false
$form.Controls.Add($errorLabel)

$actions = New-Object System.Windows.Forms.Panel
$actions.Bounds = New-Object System.Drawing.Rectangle(16, 134, 364, 30)
$actions.Visible = $false
$openButton = New-Object System.Windows.Forms.Button
$openButton.Bounds = New-Object System.Drawing.Rectangle(0, 0, 110, 28)
$openButton.Text = '打开画匣'
$openButton.FlatStyle = 'Flat'
$openButton.FlatAppearance.BorderColor = $accent
$openButton.ForeColor = $fg
$openButton.Add_Click({ Start-Process -FilePath $exePath; $form.Close() })
$actions.Controls.Add($openButton)
$dismissButton = New-Object System.Windows.Forms.Button
$dismissButton.Bounds = New-Object System.Drawing.Rectangle(118, 0, 80, 28)
$dismissButton.Text = '关闭'
$dismissButton.FlatStyle = 'Flat'
$dismissButton.FlatAppearance.BorderColor = [System.Drawing.Color]::FromArgb(58, 62, 74)
$dismissButton.ForeColor = $fg
$dismissButton.Add_Click({ $form.Close() })
$actions.Controls.Add($dismissButton)
$form.Controls.Add($actions)

# 最小化成的小条：一个标签 + 还原按钮，位置尺寸在 Set-Collapsed 里切换
$pill = New-Object System.Windows.Forms.Label
$pill.Bounds = New-Object System.Drawing.Rectangle(12, 10, 178, 24)
$pill.ForeColor = $fg
$pill.Font = New-Object System.Drawing.Font('Microsoft YaHei UI', 9)
$pill.BackColor = [System.Drawing.Color]::Transparent
$pill.Text = '画匣更新…'
$pill.AutoEllipsis = $true
$pill.Visible = $false
$form.Controls.Add($pill)

$restoreButton = New-Object System.Windows.Forms.Button
$restoreButton.Bounds = New-Object System.Drawing.Rectangle(194, 8, 32, 28)
$restoreButton.Text = [char]0x25ad
$restoreButton.FlatStyle = 'Flat'
$restoreButton.FlatAppearance.BorderColor = [System.Drawing.Color]::FromArgb(58, 62, 74)
$restoreButton.ForeColor = $fg
$restoreButton.Visible = $false
$restoreButton.Add_Click({ Set-Collapsed $false })
$form.Controls.Add($restoreButton)

# 高缩放屏：整体按物理像素放大（字体用磅值，DPI aware 下系统自己缩放）
$scaleFactor = New-Object System.Drawing.SizeF($script:s, $script:s)
$form.Scale($scaleFactor)
$normSize = New-Object System.Drawing.Size([int](396 * $script:s), [int](172 * $script:s))
$pillSize = New-Object System.Drawing.Size([int](268 * $script:s), [int](44 * $script:s))
$barFull = [int](364 * $script:s)
$normCloseRect = New-Object System.Drawing.Rectangle([int](360 * $script:s), [int](8 * $script:s), [int](28 * $script:s), [int](24 * $script:s))
$pillCloseRect = New-Object System.Drawing.Rectangle([int](232 * $script:s), [int](8 * $script:s), [int](28 * $script:s), [int](28 * $script:s))

$script:collapsed = $false
function Set-Collapsed {
    param([bool]$On)
    $script:collapsed = $On
    if ($On) {
        foreach ($c in @($title, $barBack, $stage, $meta, $errorLabel, $actions, $minButton)) { $c.Visible = $false }
        $pill.Visible = $true
        $restoreButton.Visible = $true
        $closeButton.Bounds = $pillCloseRect
        $form.Size = $pillSize
    } else {
        $pill.Visible = $false
        $restoreButton.Visible = $false
        $closeButton.Bounds = $normCloseRect
        foreach ($c in @($title, $barBack, $stage, $meta, $minButton)) { $c.Visible = $true }
        $form.Size = $normSize
    }
}

# 拖动：窗体本体和所有会吞鼠标事件的标签/面板都挂上模拟标题栏处理
$dragHandler = {
    param($sender, $e)
    if ($e.Button -eq [System.Windows.Forms.MouseButtons]::Left) {
        [void][HudNative.Drag]::ReleaseCapture()
        [void][HudNative.Drag]::SendMessage($form.Handle, 0xA1, [IntPtr]2, [IntPtr]0)
    }
}
$form.Add_MouseDown($dragHandler)
foreach ($c in @($title, $stage, $meta, $pill, $barBack, $bar, $errorLabel)) { $c.Add_MouseDown($dragHandler) }

$startedAt = $null
$closing = $false
$timer = New-Object System.Windows.Forms.Timer
$timer.Interval = 500
$timer.Add_Tick({
    try {
        if (-not (Test-Path -LiteralPath $progressPath)) { return }
        $data = Get-Content -LiteralPath $progressPath -Raw -Encoding UTF8 | ConvertFrom-Json
        if ($data.startedAt) { $script:startedAt = [int64]$data.startedAt }
        $elapsedText = '00:00'
        if ($script:startedAt) {
            $elapsed = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() - $script:startedAt
            if ($elapsed -lt 0) { $elapsed = 0 }
            $elapsedText = '{0:mm\:ss}' -f [TimeSpan]::FromMilliseconds($elapsed)
        }
        if ($null -eq $data) { return }

        $result = ''
        if ($data.result) { $result = [string]$data.result }
        $pct = $null
        $etaText = ''
        $pillText = ''
        if ($data.version) { $title.Text = ('画匣 · 正在更新 v' + $data.version) }

        if ($result -eq 'success') {
            $bar.BackColor = $accent
            $stage.Text = '更新完成，新版已安装'
            $pillText = '更新完成'
            if (-not $script:collapsed) { $actions.Visible = $true }
            if (-not $script:closing) {
                $script:closing = $true
                $autoClose = New-Object System.Windows.Forms.Timer
                $autoClose.Interval = 20000
                $autoClose.Add_Tick({ $form.Close() })
                $autoClose.Start()
            }
        } elseif ($result -eq 'failed') {
            $bar.BackColor = $danger
            $stage.Text = '更新失败（不影响当前安装的版本）'
            $errorLabel.Text = [string]$data.error
            $errorLabel.Visible = $true
            $pillText = '更新失败'
            if (-not $script:collapsed) { $actions.Visible = $true }
        } else {
            $index = 0
            if ($data.stageIndex) { $index = [int]$data.stageIndex }
            $total = 7
            if ($data.stageTotal) { $total = [int]$data.stageTotal }
            $stageName = ''
            if ($data.stage) { $stageName = [string]$data.stage }
            if ($data.totalExpectedSec -and $data.stageExpectedSec -and $data.stageStartedAt) {
                # 加权进度：已完成阶段的预计时长 + 本步内已用时间（超过预计就封顶再缓慢爬行）
                $nowMs = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
                $elapsedStage = $nowMs - [int64]$data.stageStartedAt
                if ($elapsedStage -lt 0) { $elapsedStage = 0 }
                $stageExpMs = [double]$data.stageExpectedSec * 1000
                $doneSec = [double]$data.doneExpectedSec
                $totalSec = [double]$data.totalExpectedSec
                $effStageMs = $elapsedStage
                if ($effStageMs -gt $stageExpMs) { $effStageMs = $stageExpMs }
                $frac = ($doneSec * 1000 + $effStageMs) / ($totalSec * 1000)
                if ($elapsedStage -gt $stageExpMs) {
                    $overMs = $elapsedStage - $stageExpMs
                    $extraMs = [Math]::Min($totalSec * 30, $stageExpMs * 0.25 * (1.0 - [Math]::Exp(-$overMs / 120000.0)))
                    $frac = $frac + $extraMs / ($totalSec * 1000)
                    $etaText = '（本步比平时慢，仍在进行）'
                }
                $pct = [Math]::Round($frac * 100)
                if ($pct -gt 96) { $pct = 96 }
                if ($pct -lt 1) { $pct = 1 }
                if ($etaText -eq '') {
                    $remainSec = $totalSec - $doneSec - $effStageMs / 1000
                    if ($remainSec -lt 0) { $remainSec = 0 }
                    if ($remainSec -ge 60) {
                        $etaText = '预计剩约 {0} 分' -f [Math]::Round($remainSec / 60)
                    } else {
                        $etaText = '预计剩约 {0} 秒' -f [Math]::Round($remainSec)
                    }
                }
                $stage.Text = ('第 {0}/{1} 步 · {2}' -f ($index + 1), $total, $stageName)
            } else {
                $pct = [Math]::Min(96, [Math]::Round(($index / $total) * 100))
                $stage.Text = $stageName
            }
            $pillText = ('{0}%' -f $pct)
        }

        if ($script:collapsed) {
            if ($pillText -ne '') { $pill.Text = ('画匣 {0} · {1}' -f $pillText, $elapsedText) }
        } else {
            if ($result -eq 'success' -or $result -eq 'failed') {
                $bar.Width = $barFull
            } elseif ($pct -ne $null) {
                $bar.Width = [Math]::Max(2, [int]($barFull * $pct / 100))
            }
            if ($etaText -ne '') {
                $meta.Text = ('已用 {0} · {1}' -f $elapsedText, $etaText)
            } else {
                $meta.Text = ('已用 {0}' -f $elapsedText)
            }
        }
        $form.TopMost = $true
    } catch {}
})
$timer.Start()

# 右下角定位：等 Scale 之后的最终尺寸再算
$work = [System.Windows.Forms.Screen]::PrimaryScreen.WorkingArea
$form.Location = New-Object System.Drawing.Point(
    ($work.Right - $form.Width - [int](16 * $script:s)),
    ($work.Bottom - $form.Height - [int](16 * $script:s)))

$form.Add_Shown({ $form.TopMost = $true; $form.Activate() })
[void]$form.ShowDialog()
"##;
    template
        .replace("__PROGRESS_PATH__", &ps_quote(progress_json))
        .replace("__EXE_PATH__", &ps_quote(installed_exe))
}

/// PowerShell 5.1 按 ANSI 解读无 BOM 的 UTF-8，中文会碎——脚本必须带 BOM 落盘。
fn write_utf8_bom(path: &Path, source: &str) -> Result<(), String> {
    let mut bytes: Vec<u8> = vec![0xEF, 0xBB, 0xBF];
    bytes.extend(source.as_bytes());
    fs::write(path, bytes).map_err(|error| format!("写入更新进度窗失败: {error}"))
}

fn write_launcher(launcher: &Path, script: &Path) -> Result<(), String> {
    let source = launcher_source(script);
    let mut bytes: Vec<u8> = vec![0xFF, 0xFE]; // UTF-16LE BOM
    bytes.extend(source.encode_utf16().flat_map(|unit| unit.to_le_bytes()));
    fs::write(launcher, bytes).map_err(|error| format!("写入隐藏启动器失败: {error}"))
}

/// 安排一次「退出后发布」。
///
/// 返回（脚本路径, 日志路径）。前端拿到后应立刻退出应用。
pub fn schedule_of(app: &AppHandle, settings: &ReleaseSettings) -> Result<(String, String), String> {
    let source_raw = settings
        .source_dir
        .as_deref()
        .ok_or_else(|| "还没有设置源码目录".to_string())?;
    let source = validate_source_dir(source_raw)?;

    let dir = release_dir(app)?;
    let exe = std::env::current_exe().map_err(|error| format!("无法定位本程序: {error}"))?;
    // 编排脚本用 node 跑（见 orchestrator_source 的注释）：不再生成 .cmd——
    // 那种含 `taskkill` 裸文本的批处理会被火绒的行为检测静默删掉。
    let script = dir.join("release-on-exit.mjs");
    let log = dir.join("release.log");
    let status = dir.join("status.json");
    let pending = dir.join("pending.json");
    let progress = dir.join("progress.json");
    let hud_script = dir.join("release-progress.ps1");

    // 悬浮进度窗：release.mjs 每阶段写 progress.json，HUD 置顶读它画进度条。
    write_utf8_bom(
        &hud_script,
        &progress_hud_source(
            &progress.to_string_lossy(),
            &exe.to_string_lossy(),
        ),
    )?;

    let body = orchestrator_source(&exe, &source, &log, &status, &pending, &hud_script, settings.relaunch_after);
    fs::write(&script, body.as_bytes()).map_err(|error| format!("写入更新编排脚本失败: {error}"))?;

    // 隐藏启动器：explorer 拉它而不是直接拉 node，关窗时不弹黑框。
    let launcher = dir.join("release-on-exit.vbs");
    write_launcher(&launcher, &script)?;

    // 清掉上一轮的日志尾巴，避免面板读到旧结果。
    let _ = fs::remove_file(&log);
    let _ = fs::remove_file(&status);
    // 进度文件同理：HUD 在 npm run release 写入前显示「正在启动更新…」。
    let _ = fs::remove_file(&progress);

    // pending 标记：有它才代表「已经排好一次发布」。
    write_json(
        &pending_file(app)?,
        &serde_json::json!({
            "sourceDir": source.to_string_lossy(),
            "relaunch": settings.relaunch_after,
            "at": crate::fsutil::now_ms(),
        }),
    )?;

    Ok((
        script.to_string_lossy().to_string(),
        log.to_string_lossy().to_string(),
    ))
}

/// 拉起**隐藏启动器**（`release-on-exit.vbs`），再由它以隐藏窗口跑真正的 `.cmd`。
///
/// 两条路，按「别卡关窗」优先：
/// 1. 直接 `wscript.exe` + `DETACHED_PROCESS | CREATE_NO_WINDOW`：不弹窗、
///    与本进程脱钩、CreateProcess 基本立刻返回；
/// 2. 兜底 `explorer.exe <vbs>`：父进程变成资源管理器，最稳，但 explorer 要做一次
///    shell 激活，机器忙时能把调用方卡住好几秒——这正是「关窗停几秒才消失」的来源，
///    所以只当兜底。
///
/// 不能直接 `Command::new("cmd").spawn()`：那样脚本是本进程的子进程，
/// 本进程退出时它可能被一起带走（进程组/作业对象），而这脚本的第一件事正是
/// 等本进程退出。
pub fn launch_detached(script: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;

        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

        let launcher = script.with_extension("vbs");
        let base = CREATE_NO_WINDOW | DETACHED_PROCESS;
        // 带 BREAKAWAY 更彻底；某些作业对象不允许 breakout 会直接失败，那就退回不带它。
        for flags in [base | CREATE_BREAKAWAY_FROM_JOB, base] {
            if std::process::Command::new("wscript.exe")
                .arg(&launcher)
                .creation_flags(flags)
                .spawn()
                .is_ok()
            {
                return Ok(());
            }
        }

        std::process::Command::new("explorer.exe")
            .arg(&launcher)
            .spawn()
            .map_err(|error| format!("无法启动更新脚本: {error}"))?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = script;
        Err("当前平台不支持退出后自动发布".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{decide_has_changes, has_real_changes};

    #[test]
    fn version_bump_alone_is_not_a_change() {
        // 发布脚本自己抬版本产生的脏，不能算「有新东西要发」，否则会自激
        let porcelain = " M package.json\n M src-tauri/Cargo.lock\n M src-tauri/Cargo.toml\n M src-tauri/tauri.conf.json\n";
        assert!(!has_real_changes(porcelain));
    }

    #[test]
    fn real_source_edit_is_a_change() {
        assert!(has_real_changes(" M src-tauri/src/d2.rs\n M package.json\n"));
        assert!(has_real_changes("?? src-tauri/src/new_module.rs\n"));
        assert!(has_real_changes("M  src/components/D2Panel.tsx\n"));
    }

    #[test]
    fn rename_takes_the_new_path() {
        // 重命名到版本文件之外，仍算改动
        assert!(has_real_changes("R  src/old.ts -> src/new.ts\n"));
    }

    #[test]
    fn empty_porcelain_is_clean() {
        assert!(!has_real_changes(""));
        assert!(!has_real_changes("   \n"));
    }

    #[test]
    fn no_baseline_allows_first_release() {
        // 从没发布过：第一次关窗应该允许发布并建立基线
        assert!(decide_has_changes("", Some("abc123"), Some(false)));
    }

    #[test]
    fn new_commit_means_update() {
        assert!(decide_has_changes("oldhead", Some("newhead"), Some(false)));
    }

    #[test]
    fn dirty_tree_means_update() {
        assert!(decide_has_changes("samehead", Some("samehead"), Some(true)));
    }

    #[test]
    fn clean_and_same_head_means_no_update() {
        // 核心：关一次不等于更新一次——这正是管理员反馈的 bug
        assert!(!decide_has_changes("samehead", Some("samehead"), Some(false)));
    }

    #[test]
    fn git_unavailable_is_conservative() {
        assert!(decide_has_changes("samehead", None, Some(false)));
        assert!(decide_has_changes("samehead", Some("samehead"), None));
    }
}
