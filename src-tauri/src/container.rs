//! 统一压缩包容器抽象。
//!
//! - ZIP/CBZ：随机访问直读（原有路径）；
//! - RAR/CBR：每次读取重新打开归档、顺序游标 skip 到目标条目再读入内存；
//!   大多数漫画包是 STORED 无压缩，跳过只解析头部，代价很小；
//! - 7z/CB7：solid 分块无法随机访问，首次读取时整体解包到临时缓存目录，
//!   之后按磁盘文件直读。缓存目录按「路径+修改时间+大小」寻址，7 天未动
//!   的目录在下次解包时顺手清理。
//!
//! `list_entries` 与 `read_entry` 的 `raw_index` 语义一致：容器内第 N 个
//! 「文件条目」（目录不计入）的序号，顺序由容器自身决定，跨调用稳定。

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use zip::ZipArchive;

use crate::fsutil;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContainerKind {
    Zip,
    SevenZ,
    Rar,
}

pub(crate) fn kind_for(path: &Path) -> Option<ContainerKind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "zip" | "cbz" => Some(ContainerKind::Zip),
        "7z" | "cb7" => Some(ContainerKind::SevenZ),
        "rar" | "cbr" => Some(ContainerKind::Rar),
        _ => None,
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ContainerEntry {
    pub name: String,
    pub size: u64,
    pub raw_index: usize,
}

/// 7z 解包缓存的保留时长。
const CONTAINER_CACHE_TTL: Duration = Duration::from_secs(7 * 24 * 3600);

fn containers_root() -> PathBuf {
    std::env::temp_dir().join("archive-gallery-containers")
}

/// 7z 解包互斥：并发读同一包时只允许一次解包，其余等结果。
fn extract_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn is_encryption_error(message: &str) -> bool {
    message.contains("password") || message.contains("ncrypt")
}

fn open_error(kind: &str, error: impl std::fmt::Display) -> String {
    let message = error.to_string();
    if is_encryption_error(&message) {
        return format!("{kind} 压缩包已加密，暂不支持");
    }
    format!("压缩包损坏或格式不支持: {message}")
}

fn read_error(error: impl std::fmt::Display) -> String {
    let message = error.to_string();
    if is_encryption_error(&message) {
        return "压缩包已加密，暂不支持".to_string();
    }
    format!("读取页面失败: {message}")
}

/// 一次性列出包内所有文件条目（不解包、不写缓存）。
/// 注意：7z solid 包为枚举要付一次全量解码的 CPU 成本，这是格式本身决定的。
pub(crate) fn list_entries(path: &Path) -> Result<Vec<ContainerEntry>, String> {
    match kind_for(path) {
        Some(ContainerKind::Zip) => list_zip(path),
        Some(ContainerKind::SevenZ) => list_sevenz(path),
        Some(ContainerKind::Rar) => list_rar(path),
        None => Err("不支持的压缩包格式".to_string()),
    }
}

/// 按 `list_entries` 输出的条目序号读出整条字节。调用方负责大小/资源上限。
pub(crate) fn read_entry(path: &Path, raw_index: usize) -> Result<Vec<u8>, String> {
    read_entry_with_password(path, raw_index, None)
}

/// 同 `read_entry`，带解压密码。
///
/// 加密支持范围（11-23）：**ZIP/CBZ 全支持**（ZipCrypto 与 AES 都由 zip crate 处理）；
/// 7z / RAR 加密暂不支持，给出明确提示而不是笼统的「损坏」。
pub(crate) fn read_entry_with_password(
    path: &Path,
    raw_index: usize,
    password: Option<&str>,
) -> Result<Vec<u8>, String> {
    match kind_for(path) {
        Some(ContainerKind::Zip) => read_zip(path, raw_index, password),
        Some(ContainerKind::SevenZ) => {
            if password.map(|value| !value.is_empty()).unwrap_or(false) {
                return Err("7z 加密暂不支持：请先解压后重新打包成 ZIP/CBZ".to_string());
            }
            read_sevenz(path, raw_index)
        }
        Some(ContainerKind::Rar) => {
            if password.map(|value| !value.is_empty()).unwrap_or(false) {
                return Err("RAR 加密暂不支持：请先解压后重新打包成 ZIP/CBZ".to_string());
            }
            read_rar(path, raw_index)
        }
        None => Err("不支持的压缩包格式".to_string()),
    }
}

fn list_zip(path: &Path) -> Result<Vec<ContainerEntry>, String> {
    let file = File::open(path).map_err(|error| format!("无法打开压缩包: {error}"))?;
    let mut archive = ZipArchive::new(file).map_err(|error| open_error("ZIP", error))?;

    let mut entries = Vec::new();
    for index in 0..archive.len() {
        let Ok(entry) = archive.by_index(index) else {
            continue;
        };
        if entry.is_dir() {
            continue;
        }
        entries.push(ContainerEntry {
            name: entry.name().to_string(),
            size: entry.size(),
            raw_index: entries.len(),
        });
    }
    Ok(entries)
}

fn read_zip(path: &Path, raw_index: usize, password: Option<&str>) -> Result<Vec<u8>, String> {
    let file = File::open(path).map_err(|error| format!("无法打开压缩包: {error}"))?;
    let mut archive = ZipArchive::new(file).map_err(|error| open_error("ZIP", error))?;
    let secret = password.filter(|value| !value.is_empty());

    let mut seen = 0usize;
    for index in 0..archive.len() {
        let mut entry = match archive.by_index(index) {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        if entry.is_dir() {
            continue;
        }
        if seen == raw_index && entry.encrypted() {
            // 加密条目：用给定密码重开一次（by_index 拿到的是未解密句柄）。
            drop(entry);
            let Some(secret) = secret else {
                return Err("加密压缩包：请在导入时填写密码".to_string());
            };
            let mut decrypted = archive
                .by_index_decrypt(index, secret.as_bytes())
                .map_err(|_| "密码不正确（或该包不是 ZIP 加密格式）".to_string())?;
            let mut bytes = Vec::new();
            decrypted
                .read_to_end(&mut bytes)
                .map_err(|_| "密码不正确（数据校验失败）".to_string())?;
            return Ok(bytes);
        }
        if entry.is_dir() {
            continue;
        }
        if seen == raw_index {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).map_err(read_error)?;
            return Ok(bytes);
        }
        seen += 1;
    }
    Err("页码超出范围".to_string())
}

fn list_sevenz(path: &Path) -> Result<Vec<ContainerEntry>, String> {
    let mut reader = sevenz_rust2::ArchiveReader::open(path, sevenz_rust2::Password::empty())
        .map_err(|error| open_error("7z", error))?;
    let mut entries = Vec::new();
    reader
        .for_each_entries(|entry, _data| {
            if !entry.is_directory() {
                entries.push(ContainerEntry {
                    name: entry.name().to_string(),
                    size: entry.size(),
                    raw_index: entries.len(),
                });
            }
            Ok(true)
        })
        .map_err(|error| format!("读取压缩包条目失败: {error}"))?;
    Ok(entries)
}

fn sevenz_cache_dir(path: &Path) -> Result<PathBuf, String> {
    let metadata = fs::metadata(path).map_err(|error| format!("无法读取压缩包信息: {error}"))?;
    let stamp = format!(
        "{}|{}|{}",
        path.to_string_lossy(),
        fsutil::modified_ms(&metadata),
        metadata.len()
    );
    Ok(containers_root().join(&blake3::hash(stamp.as_bytes()).to_hex()[..20]))
}

/// 确保包已解包到缓存目录并返回目录路径。内部文件按到达顺序命名为
/// `000000.bin`、`000001.bin`……与 `list_sevenz` 的 raw_index 一一对应。
/// 只有完整解包的目录才会落到正式位置（先写 `.tmp-*` 再原子 rename），
/// 中断不会留下半成品。
fn ensure_sevenz_extracted(path: &Path) -> Result<PathBuf, String> {
    let dir = sevenz_cache_dir(path)?;
    if dir.is_dir() {
        return Ok(dir);
    }

    let _guard = extract_lock().lock().map_err(|_| "解包锁异常".to_string())?;
    // 拿到锁后再查一次：并发路径下可能已被别的线程解好。
    if dir.is_dir() {
        return Ok(dir);
    }

    prune_containers(&dir);

    let temp = containers_root().join(format!(
        "{}.tmp-{}",
        dir.file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("cache"),
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&temp);
    fs::create_dir_all(&temp).map_err(|error| format!("无法创建解包缓存目录: {error}"))?;

    let result = (|| -> Result<(), String> {
        let mut reader = sevenz_rust2::ArchiveReader::open(path, sevenz_rust2::Password::empty())
            .map_err(|error| open_error("7z", error))?;
        let mut count = 0usize;
        let mut write_failure: Option<String> = None;
        reader
            .for_each_entries(|entry, data| {
                if write_failure.is_some() {
                    return Ok(false);
                }
                if entry.is_directory() {
                    return Ok(true);
                }
                let target = temp.join(format!("{count:06}.bin"));
                count += 1;
                let outcome = (|| -> io::Result<()> {
                    let mut output = File::create(&target)?;
                    io::copy(data, &mut output)?;
                    Ok(())
                })();
                if let Err(error) = outcome {
                    let _ = fs::remove_file(&target);
                    write_failure = Some(format!("解包缓存写入失败: {error}"));
                    return Ok(false);
                }
                Ok(true)
            })
            .map_err(|error| format!("读取压缩包条目失败: {error}"))?;
        if let Some(error) = write_failure {
            return Err(error);
        }
        Ok(())
    })();

    if let Err(error) = result {
        let _ = fs::remove_dir_all(&temp);
        return Err(error);
    }

    let _ = fs::remove_dir_all(&dir);
    fs::rename(&temp, &dir).map_err(|error| format!("解包缓存落位失败: {error}"))?;
    Ok(dir)
}

fn read_sevenz(path: &Path, raw_index: usize) -> Result<Vec<u8>, String> {
    let dir = ensure_sevenz_extracted(path)?;
    fs::read(dir.join(format!("{raw_index:06}.bin"))).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            "页码超出范围".to_string()
        } else {
            format!("读取页面失败: {error}")
        }
    })
}

/// 清理 7 天未动的其他解包缓存目录（当前目录除外）。
fn prune_containers(keep: &Path) {
    let Ok(items) = fs::read_dir(containers_root()) else {
        return;
    };
    let now = SystemTime::now();
    for item in items.flatten() {
        let path = item.path();
        if path == keep {
            continue;
        }
        let stale = item
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > CONTAINER_CACHE_TTL);
        if stale {
            let _ = fs::remove_dir_all(&path);
        }
    }
}

fn rar_error(error: impl std::fmt::Display) -> String {
    let message = error.to_string();
    if message.contains("assword") || message.contains("ncrypt") {
        return "RAR 压缩包已加密，暂不支持".to_string();
    }
    format!("RAR 读取失败: {message}")
}

fn list_rar(path: &Path) -> Result<Vec<ContainerEntry>, String> {
    let handle = unrar_ng::Archive::new(path);
    if handle.is_multipart() {
        return Err("分卷 RAR 暂不支持".to_string());
    }
    let mut archive = handle
        .open_for_listing()
        .map_err(|error| open_error("RAR", error))?;

    let mut entries = Vec::new();
    loop {
        let next = archive.read_header().map_err(rar_error)?;
        let Some(current) = next else {
            break;
        };
        let header = current.entry();
        if header.is_file() {
            entries.push(ContainerEntry {
                name: header.filename.to_string_lossy().to_string(),
                size: header.unpacked_size,
                raw_index: entries.len(),
            });
        }
        archive = current.skip().map_err(rar_error)?;
    }
    Ok(entries)
}

fn read_rar(path: &Path, raw_index: usize) -> Result<Vec<u8>, String> {
    let mut archive = unrar_ng::Archive::new(path)
        .open_for_processing()
        .map_err(|error| open_error("RAR", error))?;

    let mut seen = 0usize;
    loop {
        let next = archive.read_header().map_err(rar_error)?;
        let Some(current) = next else {
            return Err("页码超出范围".to_string());
        };
        if !current.entry().is_file() {
            archive = current.skip().map_err(rar_error)?;
            continue;
        }
        if seen == raw_index {
            let (bytes, _rest) = current.read().map_err(rar_error)?;
            return Ok(bytes);
        }
        seen += 1;
        archive = current.skip().map_err(rar_error)?;
    }
}
