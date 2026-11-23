use std::fs::{self, File, Metadata};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn modified_ms(metadata: &Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn hash_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| format!("无法读取文件: {e}"))?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0u8; 1024 * 1024];

    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|e| format!("读取文件失败: {e}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    Ok(hasher.finalize().to_hex().to_string())
}

pub fn copy_and_hash(source: &Path, target: &Path) -> Result<(String, u64), String> {
    let mut source_file = File::open(source).map_err(|e| format!("无法读取源文件: {e}"))?;

    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("无法创建临时目录: {e}"))?;
    }

    let mut target_file = File::create(target).map_err(|e| format!("无法创建临时文件: {e}"))?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    let mut total = 0u64;

    loop {
        let read = source_file
            .read(&mut buffer)
            .map_err(|e| format!("读取源文件失败: {e}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        target_file
            .write_all(&buffer[..read])
            .map_err(|e| format!("写入临时文件失败: {e}"))?;
        total += read as u64;
    }

    target_file
        .flush()
        .map_err(|e| format!("写入临时文件失败: {e}"))?;

    Ok((hasher.finalize().to_hex().to_string(), total))
}

pub fn sanitize_name(input: &str, directory: bool) -> String {
    let mut sanitized = String::with_capacity(input.len());
    for character in input.chars() {
        if character.is_control()
            || matches!(
                character,
                '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
            )
        {
            sanitized.push('_');
        } else {
            sanitized.push(character);
        }
    }

    let mut sanitized = sanitized
        .trim()
        .trim_end_matches(|character| character == '.' || character == ' ')
        .to_string();

    if sanitized.is_empty() || sanitized == "." || sanitized == ".." {
        return if directory {
            "未命名目录".to_string()
        } else {
            "未命名".to_string()
        };
    }

    let stem = sanitized
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    if matches!(
        stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    ) {
        sanitized = format!("_{sanitized}");
    }

    sanitized
}

pub fn rel_to_string(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let mut parts = Vec::new();

    for component in relative.components() {
        match component {
            Component::Normal(value) => parts.push(value.to_string_lossy().to_string()),
            _ => return None,
        }
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

pub fn join_rel(root: &Path, relative: &str) -> Option<PathBuf> {
    let mut path = root.to_path_buf();
    for part in relative.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return None;
        }
        path.push(part);
    }
    Some(path)
}

pub fn path_with_suffix(path: &Path, suffix: usize) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let stem = path
        .file_stem()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| "未命名".to_string());
    let extension = path
        .extension()
        .map(|value| value.to_string_lossy().to_string());

    let name = match extension {
        Some(extension) if !extension.is_empty() => format!("{stem} ({suffix}).{extension}"),
        _ => format!("{stem} ({suffix})"),
    };

    parent.join(name)
}
