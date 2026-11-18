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

/// 用 explorer.exe 拉起脚本。
///
/// 不能直接 `Command::new("cmd").spawn()`：那样脚本是本进程的子进程，
/// 本进程退出时它可能被一起带走（进程组/作业对象），而这脚本的第一件事正是
/// 等本进程退出。交给 explorer 启动，父进程变成资源管理器，与本进程彻底脱钩。
pub fn launch_detached(script: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        std::process::Command::new("explorer.exe")
            .arg(script)
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
