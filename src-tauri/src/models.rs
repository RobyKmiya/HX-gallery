use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryState {
    pub library_path: Option<String>,
    pub library_name: Option<String>,
    pub archive_count: i64,
    pub missing_count: i64,
    pub total_size: i64,
    pub last_library_path: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveCard {
    pub id: i64,
    pub title: String,
    pub file_name: String,
    pub ext: String,
    pub file_size: i64,
    pub rating: i64,
    pub page_count: i64,
    pub added_at: i64,
    pub file_mtime: i64,
    pub cover_path: Option<String>,
    pub file_path: String,
    pub note: Option<String>,
    pub missing: bool,
    pub error: Option<String>,
    pub last_read_at: Option<i64>,
    pub last_page: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_bucket: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_category: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveGroup {
    pub bucket: i64,
    pub count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagCategory {
    pub id: i64,
    pub name: String,
    pub parent_id: Option<i64>,
    pub path: String,
    pub depth: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchivePage {
    pub items: Vec<ArchiveCard>,
    pub total: i64,
    pub page: u32,
    pub page_size: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub groups: Option<Vec<ArchiveGroup>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReaderInfo {
    pub page_count: i64,
    pub last_page: i64,
    pub bookmarks: Vec<ReaderBookmark>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReaderBookmark {
    pub id: i64,
    pub archive_id: i64,
    pub page: i64,
    pub created_at: i64,
}

/// 单页元信息。
///
/// 图片本身不再随 IPC 返回：`read_reader_page` 只负责把页面字节解码/转码后
/// 放进后端缓存，前端用 `gallery://localhost/page/<archiveId>/<index>` 取图。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReaderPage {
    pub index: i64,
    pub name: String,
}

/// 缩略图。
///
/// 缩略图只有十几 KB，继续直接内联成 data URL：省一次协议往返，
/// 也能复用现有的懒加载 + JS 侧缓存逻辑。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReaderThumb {
    pub index: i64,
    pub name: String,
    pub data_url: String,
}

/// 画集列表查询参数。
///
/// 这些字段全部来自前端 IPC（运行时 JSON），因此：
/// - 开启 `deny_unknown_fields`：字段名拼错（如 pageSize 写成 pagesize）会立刻报错，
///   而不是静默退回默认值、表现成「筛选没生效」；
/// - 数量与长度上限在 `library::query_archives` 里统一夹紧，避免超长 `IN (...)`、
///   超长 FTS 短语与超深 OFFSET。
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArchiveQuery {
    pub page: Option<u32>,
    pub page_size: Option<u32>,
    pub sort: Option<String>,
    pub direction: Option<String>,
    pub collection_id: Option<i64>,
    pub tag_ids: Option<Vec<i64>>,
    pub tag_mode: Option<String>,
    pub rating_buckets: Option<Vec<i64>>,
    pub read_statuses: Option<Vec<String>>,
    pub missing_only: Option<bool>,
    pub search: Option<String>,
    pub group: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ImportOptions {
    pub add_to_collection: Option<i64>,
    pub preserve_structure: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportProgress {
    pub current: usize,
    pub total: usize,
    pub file_name: String,
    pub imported: usize,
    pub skipped: usize,
    pub failed: usize,
    pub phase: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportErrorItem {
    pub file: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub imported: usize,
    pub skipped: usize,
    pub failed: usize,
    pub errors: Vec<ImportErrorItem>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tag {
    pub id: i64,
    pub number: i64,
    pub name: String,
    pub original_name: String,
    pub translated_name: Option<String>,
    pub translation_source: Option<String>,
    pub translated_at: Option<i64>,
    pub category: Option<String>,
    pub merged_into: Option<i64>,
    pub deleted_at: Option<i64>,
    pub count: i64,
}

/// 批量导入时的一条标签：名字 + 可选大类（已由前端把 `female:` 这类
/// 命名空间映射成「女性」这种大类路径）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagDraft {
    pub name: String,
    #[serde(default)]
    pub category: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BulkTagFailure {
    pub name: String,
    pub reason: String,
}

/// 批量导入结果。单条失败不会中断整批，所以这里按「成功多少 + 失败明细」回传。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BulkTagReport {
    /// 本次新建（含从回收站恢复）的标签数。
    pub added: i64,
    /// 真正挂到这本画集上的数量。
    pub linked: i64,
    /// 库里本来就有的标签数。
    pub existing: i64,
    /// 顺带补齐了大类的标签数（只填空缺，不覆盖已有分类）。
    pub categorized: i64,
    pub failed: Vec<BulkTagFailure>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagTranslationInput {
    pub id: i64,
    pub translated_name: Option<String>,
    pub source: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagEdge {
    pub source: i64,
    pub target: i64,
    pub weight: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagGraph {
    pub nodes: Vec<Tag>,
    pub edges: Vec<TagEdge>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashedArchive {
    pub id: i64,
    pub title: String,
    pub file_name: String,
    pub ext: String,
    pub file_size: i64,
    pub page_count: i64,
    pub last_read_at: Option<i64>,
    pub last_page: i64,
    pub missing: bool,
    pub cover_path: Option<String>,
    pub deleted_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Collection {
    pub id: i64,
    pub name: String,
    pub is_default: bool,
    pub count: i64,
    pub sort_order: i64,
    pub cover_archive_id: Option<i64>,
    pub cover_path: Option<String>,
    pub deleted_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanReport {
    pub found: usize,
    pub added: usize,
    pub updated: usize,
    pub missing: usize,
    pub errors: Vec<String>,
    pub elapsed_ms: u128,
    pub message: String,
}
