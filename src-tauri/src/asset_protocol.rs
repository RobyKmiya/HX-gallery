//! `gallery://` 自定义协议：把阅读器页面字节直接交给 webview。
//!
//! 之前整页图片走 IPC 的 base64 字符串（单页上限 32MB → 43MB JSON），
//! 现在命令层只负责「把页面解码进缓存」，图片由这里按 URL 提供。
//!
//! 三条硬约束：
//! 1. 处理器跑在 webview 的 UI 线程上，**不能**查数据库、不能解压、不能解码；
//! 2. 路径只能来自命令层登记过的画集（`reader::remembered_archive`），
//!    绝不接受 URL 里的文件路径，避免任意文件读取；
//! 3. 缓存未命中必须立刻返回 404，让前端回到命令层重新预热，
//!    而不是在这里做重活把界面卡住。

use tauri::http::{Request, Response, StatusCode};

use crate::reader;

fn plain(status: StatusCode, message: &str) -> Response<Vec<u8>> {
    let body = message.as_bytes().to_vec();
    Response::builder()
        .status(status)
        .header("Content-Type", "text/plain; charset=utf-8")
        .header("Cache-Control", "no-store")
        .body(body)
        .unwrap_or_else(|_| Response::new(Vec::new()))
}

/// 解析 `/page/<archiveId>/<index>`。
fn parse_page_path(path: &str) -> Option<(i64, i64)> {
    let mut parts = path.trim_start_matches('/').split('/');
    if parts.next()? != "page" {
        return None;
    }

    let archive_id: i64 = parts.next()?.parse().ok()?;
    let index: i64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    if archive_id <= 0 || index < 0 {
        return None;
    }

    Some((archive_id, index))
}

/// 解析 `/frame/<archiveId>/<index>/<frame>`（动图单帧 PNG）。
fn parse_frame_path(path: &str) -> Option<(i64, i64, u32)> {
    let mut parts = path.trim_start_matches('/').split('/');
    if parts.next()? != "frame" {
        return None;
    }

    let archive_id: i64 = parts.next()?.parse().ok()?;
    let index: i64 = parts.next()?.parse().ok()?;
    let frame: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    if archive_id <= 0 || index < 0 {
        return None;
    }

    Some((archive_id, index, frame))
}

/// 从 query 里取 `v=`（压缩包版本号）。
fn query_version(query: Option<&str>) -> Option<&str> {
    query?
        .split('&')
        .find_map(|pair| pair.strip_prefix("v="))
        .filter(|value| !value.is_empty())
}

pub fn serve(
    _context: tauri::UriSchemeContext<'_, tauri::Wry>,
    request: Request<Vec<u8>>,
) -> Response<Vec<u8>> {
    let page = parse_page_path(request.uri().path());
    let frame = parse_frame_path(request.uri().path());
    let Some((archive_id, index)) = page.map(|(id, index)| (id, index)).or(frame.map(|(id, index, _)| (id, index))) else {
        return plain(StatusCode::BAD_REQUEST, "请求路径无效");
    };

    let Some(path) = reader::remembered_archive(archive_id) else {
        return plain(StatusCode::NOT_FOUND, "画集未在阅读器中打开");
    };

    let version = query_version(request.uri().query());

    // URL 带版本号时，压缩包换过就必须重新预热（否则会拿到旧图）。
    if let Some(version) = version
        && version != reader::file_version(&path)
    {
        return plain(StatusCode::NOT_FOUND, "画集文件已更新，请重新打开");
    }

    let payload = if let Some((_, _, frame_index)) = frame {
        // 帧还没预热（或已被 LRU 淘汰）：交给前端重新走一次命令。
        crate::anim::cached_frame(&path, index, frame_index)
    } else {
        reader::cached_page(&path, index)
    };
    let Some(payload) = payload else {
        return plain(
            StatusCode::NOT_FOUND,
            if frame.is_some() {
                "帧尚未准备完成"
            } else {
                "页面尚未准备完成"
            },
        );
    };

    // 带 `?v=` 的 URL 与压缩包内容一一对应，可以放心让 webview 自己缓存：
    // 动图动辄二三十兆，之前 `no-store` 意味着每次重挂 <img>（重试、切页回来）
    // 都要重走一遍协议、再 memcpy 一份到 JS 侧。
    let cache_control = if version.is_some() {
        "private, max-age=3600"
    } else {
        "no-store"
    };

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", payload.mime)
        .header("Content-Length", payload.bytes.len())
        .header("Cache-Control", cache_control)
        .body(payload.bytes.as_ref().clone())
        .unwrap_or_else(|_| Response::new(Vec::new()))
}

#[cfg(test)]
mod tests {
    use super::{parse_frame_path, parse_page_path};

    #[test]
    fn parses_only_well_formed_page_paths() {
        assert_eq!(parse_page_path("/page/12/3"), Some((12, 3)));
        assert_eq!(parse_page_path("page/1/0"), Some((1, 0)));
        assert_eq!(parse_page_path("/page/12"), None);
        assert_eq!(parse_page_path("/page/0/3"), None);
        assert_eq!(parse_page_path("/page/12/-1"), None);
        assert_eq!(parse_page_path("/page/abc/3"), None);
        assert_eq!(parse_page_path("/page/12/3/extra"), None);
        assert_eq!(parse_page_path("/thumb/12/3"), None);
        assert_eq!(parse_page_path("/page/12/3/../../../etc/passwd"), None);
    }

    #[test]
    fn parses_only_well_formed_frame_paths() {
        assert_eq!(parse_frame_path("/frame/12/3/7"), Some((12, 3, 7)));
        assert_eq!(parse_frame_path("frame/1/0/0"), Some((1, 0, 0)));
        assert_eq!(parse_frame_path("/frame/12/3"), None);
        assert_eq!(parse_frame_path("/frame/12/3/x"), None);
        assert_eq!(parse_frame_path("/frame/12/3/7/extra"), None);
        assert_eq!(parse_frame_path("/frame/0/3/7"), None);
        assert_eq!(parse_frame_path("/frame/12/-1/7"), None);
        assert_eq!(parse_frame_path("/page/12/3"), None);
    }
}
