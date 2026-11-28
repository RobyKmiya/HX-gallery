//! 诊断与健康自检（1.2.4，管理员提案 #1：把人工排障变成一键自检）。
//!
//! 检查项围绕 11-28 真实踩过的坑设计：
//! - 库连接 + 完整性（quick_check）+ 画集数
//! - 库盘可写探针（E 盘虚拟盘「新建可写、原位写拒绝」就是靠写删探针抓的）
//! - 引擎：每个存活浏览器的加速键拦截器挂载结果（快捷键三轮排障的产物）
//! - 广告拦截开关与名单规模
//! - 用户脚本（画匣盾在不在仓库里）
//! - 壁纸锁配置状态（1.2.3 配置丢失事故后，让它能被一眼看见）
//! - d2 日志体积
//!
//! 「导出诊断包」把检查结果 + d2 日志 + 脱敏后的 settings.json 打成 zip，
//! 发给任何会话/人都能从零开始排查。

use serde::Serialize;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

use crate::browser;
use crate::AppState;

#[derive(Debug, Clone, Serialize)]
pub struct HealthItem {
    pub key: String,
    pub ok: bool,
    pub detail: String,
}

/// 写删探针：验证目录真实可写（比 exists() 靠谱——只读盘也存在但写不了）。
fn probe_writable(dir: &Path) -> bool {
    let probe = dir.join(".hx-write-probe");
    let ok = fs::File::create(&probe)
        .and_then(|mut file| file.write_all(b"ok"))
        .is_ok();
    let _ = fs::remove_file(&probe);
    ok
}

fn log_path(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_cache_dir()
        .ok()
        .map(|cache| cache.join("d2").join("d2.log"))
}

pub fn run_health(app: &AppHandle) -> Vec<HealthItem> {
    let mut items: Vec<HealthItem> = Vec::new();
    let mut push = |key: &str, ok: bool, detail: String| {
        items.push(HealthItem {
            key: key.to_string(),
            ok,
            detail,
        });
    };

    // ── 库 ──
    let state = app.state::<AppState>();
    let library_info: Option<(std::path::PathBuf, i64, bool)> = match state.lock() {
        Ok(inner) => match &inner.library {
            Some(library) => {
                let quick = library
                    .conn
                    .query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))
                    .unwrap_or_else(|_| format!("查询失败"));
                let count = library
                    .conn
                    .query_row("SELECT COUNT(*) FROM archives", [], |row| {
                        row.get::<_, i64>(0)
                    })
                    .unwrap_or(-1);
                Some((library.root.clone(), count, quick == "ok"))
            }
            None => None,
        },
        Err(error) => {
            push("库状态", false, format!("状态表锁定失败：{error}"));
            None
        }
    };
    match library_info {
        Some((root, count, integrity_ok)) => {
            push(
                "库连接",
                true,
                format!("{}（{} 本画集）", root.display(), count),
            );
            push(
                "库完整性",
                integrity_ok,
                if integrity_ok {
                    "quick_check 通过".to_string()
                } else {
                    "quick_check 异常——请立即导出诊断包并备份库".to_string()
                },
            );
            let writable = probe_writable(&root);
            push(
                "库盘可写",
                writable,
                if writable {
                    "写删探针通过".to_string()
                } else {
                    "写删探针失败——盘可能只读 / 未就绪 / 挂载异常".to_string()
                },
            );
        }
        None => push("库连接", false, "库未打开".to_string()),
    }

    // ── 引擎（浏览器实例）──
    let accel = browser::accel_mount_status();
    if accel.is_empty() {
        push("匣浏览器", true, "未打开（跳过引擎检查）".to_string());
    } else {
        for (label, ok) in accel {
            push(
                &format!("引擎 {label} 加速键"),
                ok,
                if ok {
                    "原生拦截器已挂载".to_string()
                } else {
                    "挂载失败——快捷键不可用（重启浏览器可重试）".to_string()
                },
            );
        }
    }
    push(
        "广告拦截",
        true,
        format!(
            "{}（内置 {} 域 + 自定义 {} 域）",
            if browser::adblock_enabled() { "开" } else { "关" },
            browser::builtin_adblock_domains(),
            browser::custom_adblock_domain_count()
        ),
    );

    // ── 用户脚本 ──
    match crate::userscripts::store_dir(app) {
        Some(store) => {
            let scripts = crate::userscripts::list_scripts(&store);
            let shield = store.join("huaxia-shield.user.js");
            push(
                "用户脚本",
                !scripts.is_empty(),
                format!(
                    "{} 个脚本，画匣盾{}",
                    scripts.len(),
                    if shield.exists() { "就位" } else { "缺失！" }
                ),
            );
        }
        None => push("用户脚本", false, "脚本仓库目录不可用".to_string()),
    }

    // ── 壁纸锁 ──
    let settings = crate::read_settings(app);
    match &settings.wallpaper_lock {
        Some(lock) => {
            let flag_bypass = crate::wallpaper_lock_flag_path()
                .map(|path| path.exists())
                .unwrap_or(false);
            let detail = format!(
                "{} · 密码{} · 壁纸 {} 张{}",
                if lock.enabled { "开启" } else { "关闭" },
                if !lock.password_hash.is_empty() { "已设" } else { "未设" },
                lock.walls.len(),
                if flag_bypass { " · 救急旗标存在（启动会被绕过）" } else { "" }
            );
            push("壁纸锁", !flag_bypass, detail);
        }
        None => push(
            "壁纸锁",
            false,
            "未配置——设置 → 壁纸锁 选壁纸 + 设密码 + 开启".to_string(),
        ),
    }

    // ── 日志 ──
    match log_path(app) {
        Some(path) => {
            let size = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
            push(
                "d2 日志",
                size < 20 * 1024 * 1024,
                format!(
                    "{}（{} KB）",
                    path.display(),
                    size / 1024
                ),
            );
        }
        None => push("d2 日志", false, "日志目录不可用".to_string()),
    }

    items
}

/// 打包诊断：health.json + d2 日志 + 脱敏 settings.json。
/// 返回 zip 落盘路径（前端展示，用户拿去发人皆可）。
pub fn export_bundle(app: &AppHandle) -> Result<String, String> {
    let cache = app
        .path()
        .app_cache_dir()
        .map_err(|error| format!("无法获取缓存目录: {error}"))?;
    let dir = cache.join("diag");
    fs::create_dir_all(&dir).map_err(|error| format!("无法创建诊断目录: {error}"))?;
    let zip_path = dir.join(format!("diag-{}.zip", crate::fsutil::now_ms()));
    let file =
        fs::File::create(&zip_path).map_err(|error| format!("无法创建诊断包: {error}"))?;
    let mut zip = zip::ZipWriter::new(file);
    let options: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();

    let health = serde_json::to_string_pretty(&run_health(app))
        .map_err(|error| format!("序列化自检结果失败: {error}"))?;
    zip.start_file("health.json", options)
        .map_err(|error| format!("写入 health.json 失败: {error}"))?;
    zip.write_all(health.as_bytes())
        .map_err(|error| format!("写入 health.json 失败: {error}"))?;

    if let Some(path) = log_path(app) {
        if let Ok(bytes) = fs::read(&path) {
            let _ = zip.start_file("d2.log", options);
            let _ = zip.write_all(&bytes);
        }
    }

    // settings.json 脱敏：壁纸锁哈希/盐不下发。
    let settings = crate::read_settings(app);
    let mut sanitized = serde_json::to_value(&settings)
        .map_err(|error| format!("序列化设置失败: {error}"))?;
    if let Some(lock) = sanitized
        .get_mut("wallpaper_lock")
        .and_then(|value| value.as_object_mut())
    {
        lock.remove("passwordHash");
        lock.remove("password_hash");
        lock.remove("salt");
    }
    let settings_raw = serde_json::to_string_pretty(&sanitized)
        .map_err(|error| format!("序列化设置失败: {error}"))?;
    zip.start_file("settings.json", options)
        .map_err(|error| format!("写入 settings.json 失败: {error}"))?;
    zip.write_all(settings_raw.as_bytes())
        .map_err(|error| format!("写入 settings.json 失败: {error}"))?;

    zip.finish()
        .map_err(|error| format!("收尾诊断包失败: {error}"))?;
    Ok(zip_path.to_string_lossy().to_string())
}
