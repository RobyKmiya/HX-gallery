use std::path::Path;

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
    /// 打开过多少次。用来做「优先挑看得少的」加权随机。
    pub read_count: i64,
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
    /// 合集卡：只在「纯浏览」的第 1 页返回，前端置于网格最前。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series: Option<Vec<SeriesSummary>>,
}

/// 合集里的一本：集数标签、封面、页数都随成员走。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeriesItem {
    pub archive_id: i64,
    pub position: i64,
    pub label: Option<String>,
    pub title: String,
    pub cover_path: Option<String>,
    pub page_count: i64,
    pub missing: bool,
}

/// 合集（连载盒子）：挂在多本画集之上的一层分组，不持有文件。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeriesSummary {
    pub id: i64,
    pub name: String,
    pub note: Option<String>,
    /// 指定封面用哪一本；None 表示按顺序取第一本。
    pub cover_archive_id: Option<i64>,
    pub item_count: i64,
    pub items: Vec<SeriesItem>,
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
    /// 压缩包的「修改时间-大小」，前端拼进 URL（`?v=`）做缓存版本号。
    pub version: String,
    /// 这一页的字节数。前端据此决定要不要预取：动图单页能到二三十兆，
    /// 无脑预取 4 页会把正在看的那页从内存缓存里挤出去。
    pub bytes: i64,
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
    /// 只看某个待看目录里的画集。与 `collection_id` 同构，二选一使用。
    pub queue_id: Option<i64>,
    pub tag_ids: Option<Vec<i64>>,
    pub tag_mode: Option<String>,
    pub rating_buckets: Option<Vec<i64>>,
    pub read_statuses: Option<Vec<String>>,
    pub missing_only: Option<bool>,
    pub search: Option<String>,
    pub group: Option<String>,
    /// 只取这几本（合集详情按 id 取单本、深链等场景）。
    pub archive_ids: Option<Vec<i64>>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ImportOptions {
    pub add_to_collection: Option<i64>,
    pub preserve_structure: Option<bool>,
    /// 导入成功后挂到每一本新画集上的标签。`TagDraft` 与批量导入共用同一套
    /// 语义：分类不存在就建、已有分类不覆盖。
    #[serde(default)]
    pub tags: Vec<TagDraft>,
    /// 按文件单独指定的标签。命中的文件用自己这份（非空时），否则回落到 `tags`。
    /// 一次导入多个压缩包、每本标签不同时用这个。
    #[serde(default)]
    pub path_tags: Vec<PathTags>,
    /// 加密压缩包的密码：只填一行 = 所有加密包共用；多行 = 按顺序对应每个加密包。
    /// 只在 ZIP/CBZ 上生效（7z/RAR 加密会给出明确提示）。
    #[serde(default)]
    pub passwords: Vec<String>,
}

/// 「这个文件挂这些标签」。路径按前端回传的原样比对（含分隔符差异会先归一）。
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PathTags {
    pub path: String,
    #[serde(default)]
    pub tags: Vec<TagDraft>,
}

impl ImportOptions {
    /// 取这个文件该用的标签：单独指定且非空就用它，否则用全局那份。
    pub fn tags_for(&self, path: &Path) -> &[TagDraft] {
        if self.path_tags.is_empty() {
            return &self.tags;
        }
        let normalized = path.to_string_lossy().replace('\\', "/");
        for entry in &self.path_tags {
            if !entry.tags.is_empty()
                && entry.path.replace('\\', "/").eq_ignore_ascii_case(&normalized)
            {
                return &entry.tags;
            }
        }
        &self.tags
    }
}

/// 导入面板里的一行：拖进来（或选中）之后、点确认之前的状态。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StageItem {
    /// 源文件绝对路径，导入时按原样回传。
    pub path: String,
    pub file_name: String,
    /// 去掉扩展名的名字，界面上作为「标题」初值展示。
    pub stem: String,
    pub extension: String,
    pub size_bytes: i64,
    /// 压缩包里的图片张数；读不出来时为 None（不阻断导入）。
    pub page_count: Option<i64>,
    /// 库里已有同名同大小的记录 —— 大概率是重复拖入，导入时会被跳过。
    pub duplicate: bool,
    /// 文件本身就在库目录里，导入只建索引、不复制。
    pub in_library: bool,
    /// 无法导入的原因；非空时这一行是「不可导入」状态。
    pub issue: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StageReport {
    pub items: Vec<StageItem>,
    pub errors: Vec<ImportErrorItem>,
    pub total_bytes: i64,
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
    /// 本次真正入库的画集 id（按导入顺序）。导入面板用它串「导入后合成合集」。
    #[serde(default)]
    pub archive_ids: Vec<i64>,
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
    /// 待看目录随机挑选时的权重。默认 100；设为 0 表示「不想看带这个标签的画集」，
    /// 生成待看目录时带该标签的画集整本排除。
    pub weight: i64,
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
    /// 侧栏图标形状：star / heart / circle / square（纯填充色块）。
    pub icon: String,
    /// 图标填充色（#rrggbb）。
    pub color: String,
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

/// 待看目录。数量不设上限，成员按 added_at 排序（队列顺序）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Queue {
    pub id: i64,
    pub name: String,
    pub created_at: i64,
    /// 目录里的画集总数。
    pub count: i64,
    /// 其中已读完的本数，用于显示进度。
    pub finished: i64,
}

/// 生成待看目录的结果。排除原因按条数汇总，方便前端解释「为什么只挑到 7 本」。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueFillReport {
    pub queue: Queue,
    /// 本次实际加入的本数。
    pub added: i64,
    /// 候选池里被 0 权重标签排除掉的本数。
    pub excluded_by_weight: i64,
    /// 候选池大小（排除已读完后）。
    pub candidates: i64,
    /// 加入的画集标题，按挑选顺序。
    pub titles: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QueueFillOptions {
    /// 每个目录放几本。默认 10，夹紧到 1..=50。
    pub size: Option<i64>,
    /// 只在这批标签里挑（交集）。为空表示全库。
    #[serde(default)]
    pub tag_ids: Vec<i64>,
    /// 是否允许重复挑已经看过的（read_count > 0）。默认 false。
    pub include_read: Option<bool>,
}
