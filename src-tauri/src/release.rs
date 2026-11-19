//! 面板内的「发布更新」入口。
//!
//! 约束：真正的发布要 `npm run release`（Node + cargo + 源码目录），而且它必须
//! **在本进程退出之后**才能跑——脚本会结束 `archive-gallery.exe` 并覆盖它自己。
//! 所以这里只做三件事：
//!   1. 校验用户登记的源码目录（必须含 `scripts/release.mjs`）；
//!   2. 生成一个 `release-on-exit.cmd`，用 `explorer.exe` 拉起，
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
        status.script_path = dir.join("release-on-exit.cmd").to_string_lossy().to_string();
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
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
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
    let dirty = git_output(&source, &["status", "--porcelain"])
        .ok()
        .map(|s| !s.trim().is_empty());

    decide_has_changes(&baseline.head, head.as_deref(), dirty)
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

/// 退出前的收尾：判断源码有没有实质更新，有就把发布脚本排好并拉起。
///
/// 调用方保证**只能在后台线程调用**：这里会起 git 子进程、写脚本、用 explorer
/// 拉起隐藏启动器，全在主线程上做会让窗口消失得慢半拍
/// （管理员 2026-11-19 反馈「关窗卡好一会，不能秒关」）。
pub fn run_exit_release(app: &AppHandle, settings: &ReleaseSettings) {
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

/// rust 侧生成的 node 助手脚本：负责 UTF-8 日志追加、状态写入、以及**用中文路径
/// 重启应用**。批处理碰不了中文（见 `script_body` 的注释），这里全部交给 node。
fn helper_source(exe: &Path, log: &Path, status: &Path, relaunch: bool) -> String {
    let exe_json = serde_json::to_string(&exe.to_string_lossy().to_string())
        .unwrap_or_else(|_| "\"\"".to_string());
    let log_json = serde_json::to_string(&log.to_string_lossy().to_string())
        .unwrap_or_else(|_| "\"\"".to_string());
    let status_json = serde_json::to_string(&status.to_string_lossy().to_string())
        .unwrap_or_else(|_| "\"\"".to_string());

    format!(
        r#"// 由画匣生成，随一次更新任务存在。勿手改。
import fs from 'node:fs';
import {{ spawn }} from 'node:child_process';

const EXE = {exe_json};
const LOG = {log_json};
const STATUS = {status_json};
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

const [command, arg] = process.argv.slice(2);

if (command === 'log') {{
  say(arg);
}} else if (command === 'status') {{
  writeStatus(arg, arg === 'success' ? 'release finished' : 'npm run release failed');
}} else if (command === 'relaunch') {{
  if (!RELAUNCH) {{
    say('done');
    process.exit(0);
  }}
  if (!fs.existsSync(EXE)) {{
    say('fail');
    process.exit(0);
  }}
  try {{
    // detached + stdio ignore：父进程（本脚本）退出后它继续活着。
    spawn(EXE, [], {{ detached: true, stdio: 'ignore', cwd: EXE.replace(/[\\\/][^\\\/]+$/, '') }}).unref();
    say('done');
  }} catch (error) {{
    say('fail');
    console.error(String(error));
  }}
}}
"#,
    )
}
fn script_body(source_dir: &Path, helper: &Path, log: &Path, pending: &Path) -> String {
    let source = source_dir.to_string_lossy().replace('/', "\\");

    // 编码这件事踩了两次坑，最终方案如下：
    //   1. 脚本正文**只用 ASCII**，绝不出现中文文案或中文路径 —— 批处理按系统代码页
    //      解析，UTF-8 中文会被拆坏；而 `chcp 65001` 又会让 cmd 重读文件、把
    //      `setlocal` 解析成 `01`（实测）。
    //   2. 需要中文的地方（安装目录 `画匣`）交给 node：路径写进 UTF-8 的 json，
    //      脚本调 `node -e` 读出来再 `spawn`，node 不受 cmd 代码页影响。
    //   3. 日志同样交给 node 追加（UTF-8），避免 cmd 的 echo 把中文写坏。
    format!(
        "@echo off\r\n\
setlocal\r\n\
set \"SRC={source}\"\r\n\
\r\n\
call :say start\r\n\
call :say wait\r\n\
\r\n\
rem Wait up to ~60s for archive-gallery.exe to disappear.\r\n\
set /a tries=0\r\n\
:waitloop\r\n\
tasklist /FI \"IMAGENAME eq archive-gallery.exe\" /NH 2>nul | find /I \"archive-gallery.exe\" >nul\r\n\
if errorlevel 1 goto waitdone\r\n\
set /a tries+=1\r\n\
if %tries% GEQ 60 goto waittimeout\r\n\
call :sleep1\r\n\
goto waitloop\r\n\
\r\n\
:waittimeout\r\n\
call :say timeout_kill\r\n\
taskkill /IM archive-gallery.exe /F >nul 2>&1\r\n\
call :sleep2\r\n\
\r\n\
:waitdone\r\n\
call :say exited\r\n\
\r\n\
cd /d \"%SRC%\"\r\n\
if errorlevel 1 goto failed\r\n\
\r\n\
call :say build\r\n\
call npm run release >>\"{rawlog}\" 2>&1\r\n\
if errorlevel 1 goto failed\r\n\
\r\n\
call :say ok\r\n\
call :status success\r\n\
call :cleanup\r\n\
call :relaunch\r\n\
call :say done\r\n\
exit /b 0\r\n\
\r\n\
:failed\r\n\
call :say fail\r\n\
call :status failed\r\n\
call :cleanup\r\n\
call :relaunch\r\n\
exit /b 1\r\n\
\r\n\
:say\r\n\
node \"{helper}\" log %~1\r\n\
exit /b\r\n\
\r\n\
:status\r\n\
node \"{helper}\" status %~1\r\n\
exit /b\r\n\
\r\n\
:cleanup\r\n\
del \"{rawpending}\" >nul 2>&1\r\n\
exit /b\r\n\
\r\n\
:relaunch\r\n\
node \"{helper}\" relaunch\r\n\
exit /b\r\n\
\r\n\
:sleep1\r\n\
ping -n 2 127.0.0.1 >nul\r\n\
exit /b\r\n\
\r\n\
:sleep2\r\n\
ping -n 4 127.0.0.1 >nul\r\n\
exit /b\r\n",
        source = source,
        rawlog = log.to_string_lossy().replace('/', "\\"),
        rawpending = pending.to_string_lossy().replace('/', "\\"),
        helper = helper.to_string_lossy().replace('/', "\\"),
    )
}

/// explorer 直接拉 `.cmd` 会带出一个可见控制台，发布期间一挂好几分钟。
/// 这里生成一个 VBS 垫片：wscript 是 GUI 进程本身不弹窗，再以窗口样式 0
/// （隐藏、不等待）启动 cmd，整条发布链路（含 `npm run release`）都无窗口。
/// VBS 必须写成 **UTF-16LE + BOM**：wscript 按 ANSI 读无 BOM 文件，中文路径会碎。
fn launcher_source(script: &Path) -> String {
    // VBS 的字符串转义是把 `"` 翻倍，不能用 JSON 转义（反斜杠会被写成 \\）。
    let path_vbs = script.to_string_lossy().replace('"', "\"\"");
    format!(
        "' Generated by archive-gallery. Do not edit.\r\n\
Set sh = CreateObject(\"WScript.Shell\")\r\n\
sh.Run Chr(34) & \"{path_vbs}\" & Chr(34), 0, False\r\n"
    )
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
    let script = dir.join("release-on-exit.cmd");
    let helper = dir.join("release-helper.mjs");
    let log = dir.join("release.log");
    let status = dir.join("status.json");
    let pending = dir.join("pending.json");

    // node 助手：日志、状态、重启都走它，绕开 cmd 的中文/编码问题。
    let helper_code = helper_source(&exe, &log, &status, settings.relaunch_after);
    fs::write(&helper, helper_code.as_bytes())
        .map_err(|error| format!("写入更新助手失败: {error}"))?;

    let body = script_body(&source, &helper, &log, &pending);
    // 纯 ASCII 写盘：脚本正文不含中文，中文路径只出现在 helper 里。
    fs::write(&script, body.as_bytes()).map_err(|error| format!("写入更新脚本失败: {error}"))?;

    // 隐藏启动器：explorer 拉它而不是直接拉 cmd，关窗时不弹黑框。
    let launcher = dir.join("release-on-exit.vbs");
    write_launcher(&launcher, &script)?;

    // 清掉上一轮的日志尾巴，避免面板读到旧结果。
    let _ = fs::remove_file(&log);
    let _ = fs::remove_file(&status);

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
    use super::decide_has_changes;

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
