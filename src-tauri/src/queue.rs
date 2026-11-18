//! 待看目录：按标签权重加权随机挑一批画集。
//!
//! 挑选规则（对应需求）：
//! - 权重来自 `tags.weight`，默认 100。画集的权重 = 它所有标签权重的和。
//! - 权重为 0 的标签表示「不想看这个 tag 的画集」：**只要画集带任一 0 权标签，
//!   整本排除**（不是把该标签权重计 0，那样仍可能被其它标签抬起来）。
//! - 优先挑看得少的：以 `1 / (1 + read_count)` 作为次数因子，看过越多越难被挑中。
//! - 取 `tags.weight` 之和作为基数，再乘次数因子得到最终抽样权重。
//!
//! 随机数不引第三方 crate（工程没有 `rand` 依赖）：这里用 SplitMix64 ，
//! 以系统时间 + 库内 id 做种子，质量足够抽样且可复现（同种子同结果，便于测试）。

use crate::db;
use crate::fsutil;
use rusqlite::params_from_iter;
use rusqlite::types::Value;
use rusqlite::Connection;
use std::collections::HashMap;

/// 单个待看目录默认放几本。
use crate::db::{DEFAULT_QUEUE_SIZE, MAX_QUEUES};

/// 一个候选画集：id、标题、read_count、标签权重和。
#[derive(Clone)]
struct Candidate {
    id: i64,
    title: String,
    read_count: i64,
    weight: i64,
}

/// SplitMix64：小而够用的确定性伪随机源。
struct Rng {
    state: u64,
}

impl Rng {
    fn new(seed: u64) -> Self {
        Self {
            state: seed.wrapping_add(0x9E37_79B9_7F4A_7C15),
        }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// [0, 1) 区间的浮点数。
    fn next_f64(&mut self) -> f64 {
        (self.next_uni()) as f64 / (u32::MAX as f64 + 1.0)
    }

    fn next_uni(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }
}

/// 用时间做种子的默认随机源。
pub fn time_seed() -> u64 {
    let now = fsutil::now_ms() as u64;
    now ^ (now << 17) ^ 0x2545_F491_4F6C_DD1D
}

/// 取出候选池，并统计被 0 权重标签排除掉的本数。
///
/// 一次查询拿全部符合条件的画集，再按 `tags.weight` 过滤：
/// 画集的标签不多（通常十几个），在内存里算比写复杂 SQL 更清楚，
/// 也避免 `GROUP_CONCAT` 拼接后无法索引。
fn collect_candidates(
    conn: &Connection,
    tag_ids: &[i64],
    include_read: bool,
) -> Result<(Vec<Candidate>, i64), String> {
    let mut conditions = vec!["a.deleted_at IS NULL".to_string()];
    let mut params: Vec<Value> = Vec::new();

    // 只在指定标签里挑时，要求画集带其中「任一」标签即可（并集比交集更符合
    // 「挑符合这些 tag 的画集」的直觉；交集会让标签一多就挑选不出东西）。
    if !tag_ids.is_empty() {
        let list = tag_ids
            .iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        conditions.push(format!(
            "EXISTS (
                SELECT 1 FROM archive_tags at
                JOIN tags t ON t.id = at.tag_id AND t.deleted_at IS NULL
                WHERE at.archive_id = a.id AND at.tag_id IN ({list})
            )"
        ));
    }

    // 默认只要没读过的；include_read 时放开。
    if !include_read {
        conditions.push("a.read_count = 0".to_string());
    }

    let sql = format!(
        "SELECT a.id, COALESCE(NULLIF(a.title, ''), a.file_name), a.read_count
         FROM archives a
         WHERE {}
         ORDER BY a.id",
        conditions.join(" AND ")
    );

    let mut statement = conn
        .prepare(&sql)
        .map_err(|e| format!("准备候选查询失败: {e}"))?;

    let rows = statement
        .query_map(params_from_iter(params.drain(..)), |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?))
        })
        .map_err(|e| format!("查询候选失败: {e}"))?;

    let base: Vec<(i64, String, i64)> = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取候选失败: {e}"))?;

    // 一次性把「标签 id -> 权重」和「画集 id -> 标签权重列表」取出来，
    // 避免逐本查询（候选可能上千本）。
    let weights = tag_weights(conn)?;

    let mut tag_map: HashMap<i64, Vec<i64>> = HashMap::new();
    {
        let mut statement = conn
            .prepare(
                "SELECT at.archive_id, at.tag_id
                 FROM archive_tags at
                 JOIN tags t ON t.id = at.tag_id AND t.deleted_at IS NULL",
            )
            .map_err(|e| format!("准备标签关联查询失败: {e}"))?;
        let rows = statement
            .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))
            .map_err(|e| format!("查询标签关联失败: {e}"))?;
        for row in rows {
            let (archive_id, tag_id) = row.map_err(|e| format!("读取标签关联失败: {e}"))?;
            tag_map.entry(archive_id).or_default().push(tag_id);
        }
    }

    let mut candidates = Vec::new();
    let mut excluded = 0i64;

    for (id, title, read_count) in base {
        let ids = tag_map.get(&id).cloned().unwrap_or_default();

        // 任一 0 权标签 -> 整本排除。
        if ids
            .iter()
            .any(|tag_id| weights.get(tag_id).copied().unwrap_or(db::DEFAULT_TAG_WEIGHT) == 0)
        {
            excluded += 1;
            continue;
        }

        // 没挂标签的画集权重取默认值，否则会被永久埋掉。
        let weight: i64 = if ids.is_empty() {
            db::DEFAULT_TAG_WEIGHT
        } else {
            ids.iter()
                .map(|tag_id| {
                    weights
                        .get(tag_id)
                        .copied()
                        .unwrap_or(db::DEFAULT_TAG_WEIGHT)
                })
                .sum()
        };

        candidates.push(Candidate {
            id,
            title,
            read_count,
            weight,
        });
    }

    Ok((candidates, excluded))
}

fn tag_weights(conn: &Connection) -> Result<HashMap<i64, i64>, String> {
    let mut statement = conn
        .prepare("SELECT id, weight FROM tags WHERE deleted_at IS NULL")
        .map_err(|e| format!("准备标签权重查询失败: {e}"))?;

    let rows = statement
        .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))
        .map_err(|e| format!("查询标签权重失败: {e}"))?;

    let mut map = HashMap::new();
    for row in rows {
        let (id, weight) = row.map_err(|e| format!("读取标签权重失败: {e}"))?;
        map.insert(id, weight);
    }
    Ok(map)
}

/// 加权不放回抽样，取 `size` 本。
///
/// 用「权重和 + 随机点」的轮盘法逐个抽，抽中后移除该候选（不放回），
/// 保证同一批里不会重复；次数因子 `1/(1+read_count)` 让看过的本子显著降权。
fn pick_weighted(candidates: Vec<Candidate>, size: usize, seed: u64) -> Vec<Candidate> {
    let mut rng = Rng::new(seed);
    let mut pool = candidates;
    let mut picked = Vec::new();

    while picked.len() < size && !pool.is_empty() {
        let total: f64 = pool
            .iter()
            .map(|c| sampling_weight(c))
            .filter(|w| *w > 0.0)
            .sum();

        if total <= 0.0 {
            // 全部落入 0 权重（理论上已被排除），退化为等概率取剩余的。
            let index = (rng.next_uni() as usize) % pool.len();
            picked.push(pool.swap_remove(index));
            continue;
        }

        let mut point = rng.next_f64() * total;
        let mut chosen = None;
        for (index, candidate) in pool.iter().enumerate() {
            point -= sampling_weight(candidate);
            if point <= 0.0 {
                chosen = Some(index);
                break;
            }
        }

        // 浮点累加误差兜底：取最后一个正权重的。
        let index = chosen.unwrap_or_else(|| {
            pool.iter()
                .rposition(|c| sampling_weight(c) > 0.0)
                .unwrap_or(0)
        });
        picked.push(pool.swap_remove(index));
    }

    picked
}

/// 抽样权重 = 标签权重和 × 次数因子。
fn sampling_weight(candidate: &Candidate) -> f64 {
    (candidate.weight.max(0) as f64) / (1.0 + candidate.read_count.max(0) as f64)
}

/// 生成（或续满）一个待看目录。
///
/// - 已有同名目录时在其上追加，不再重复挑已收录的画集；
/// - 目录数量超过 `MAX_QUEUES` 时，淘汰创建时间最早的（连带清空其成员）。
pub fn fill_queue(
    conn: &mut Connection,
    name: &str,
    options: &crate::models::QueueFillOptions,
    seed: Option<u64>,
) -> Result<crate::models::QueueFillReport, String> {
    let size = options
        .size
        .unwrap_or(DEFAULT_QUEUE_SIZE)
        .clamp(1, 50) as usize;
    let include_read = options.include_read.unwrap_or(false);

    let name = name.trim();
    if name.is_empty() {
        return Err("待看目录名称不能为空".to_string());
    }
    if name.chars().count() > 32 {
        return Err("待看目录名称最多 32 个字符".to_string());
    }

    let (candidates, excluded) = collect_candidates(conn, &options.tag_ids, include_read)?;
    let candidates_total = candidates.len() as i64;

    // 排除已经在本目录里的，避免重复收录。
    let existing: Vec<i64> = {
        let mut statement = conn
            .prepare(
                "SELECT qa.archive_id FROM queue_archives qa
                 JOIN queues q ON q.id = qa.queue_id
                 WHERE q.name = ?1 COLLATE NOCASE",
            )
            .map_err(|e| format!("准备目录成员查询失败: {e}"))?;
        let rows = statement
            .query_map([name], |row| row.get::<_, i64>(0))
            .map_err(|e| format!("查询目录成员失败: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("读取目录成员失败: {e}"))?
    };

    let pool: Vec<Candidate> = candidates
        .into_iter()
        .filter(|candidate| !existing.contains(&candidate.id))
        .collect();

    let seed = seed.unwrap_or_else(time_seed);
    let picked = pick_weighted(pool, size, seed);

    let now = fsutil::now_ms();
    let transaction = db::begin_write(conn)?;

    // 目录不存在就建；数量上限在创建时把关。
    let queue_id: i64 = match transaction
        .query_row(
            "SELECT id FROM queues WHERE name = ?1 COLLATE NOCASE",
            [name],
            |row| row.get(0),
        )
        .ok()
    {
        Some(id) => id,
        None => {
            let count: i64 = transaction
                .query_row("SELECT COUNT(*) FROM queues", [], |row| row.get(0))
                .map_err(|e| format!("统计待看目录失败: {e}"))?;
            if count >= MAX_QUEUES {
                // 淘汰最旧的一个，连同成员（外键 CASCADE）。
                transaction
                    .execute(
                        "DELETE FROM queues WHERE id = (
                            SELECT id FROM queues ORDER BY created_at ASC, id ASC LIMIT 1
                         )",
                        [],
                    )
                    .map_err(|e| format!("淘汰最旧待看目录失败: {e}"))?;
            }
            transaction
                .execute(
                    "INSERT INTO queues(name, created_at) VALUES (?1, ?2)",
                    rusqlite::params![name, now],
                )
                .map_err(|e| format!("创建待看目录失败: {e}"))?;
            transaction.last_insert_rowid()
        }
    };

    let mut titles = Vec::new();
    for candidate in &picked {
        transaction
            .execute(
                "INSERT OR IGNORE INTO queue_archives(queue_id, archive_id, added_at)
                 VALUES (?1, ?2, ?3)",
                rusqlite::params![queue_id, candidate.id, now],
            )
            .map_err(|e| format!("加入待看目录失败: {e}"))?;
        titles.push(candidate.title.clone());
    }

    transaction
        .commit()
        .map_err(|e| format!("提交待看目录失败: {e}"))?;

    let queue = queue_by_name(conn, name)?
        .ok_or_else(|| "待看目录创建后读取失败".to_string())?;

    Ok(crate::models::QueueFillReport {
        queue,
        added: titles.len() as i64,
        excluded_by_weight: excluded,
        candidates: candidates_total,
        titles,
    })
}

/// 列出全部待看目录，附带进度（已读完本数）。
pub fn list_queues(conn: &Connection) -> Result<Vec<crate::models::Queue>, String> {
    let mut statement = conn
        .prepare(
            "SELECT q.id, q.name, q.created_at,
                    COUNT(qa.archive_id),
                    SUM(CASE WHEN a.page_count > 0 AND a.last_page >= a.page_count
                             THEN 1 ELSE 0 END)
             FROM queues q
             LEFT JOIN queue_archives qa ON qa.queue_id = q.id
             LEFT JOIN archives a ON a.id = qa.archive_id AND a.deleted_at IS NULL
             GROUP BY q.id
             ORDER BY q.created_at DESC, q.id DESC",
        )
        .map_err(|e| format!("准备待看目录查询失败: {e}"))?;

    let rows = statement
        .query_map([], |row| {
            Ok(crate::models::Queue {
                id: row.get(0)?,
                name: row.get(1)?,
                created_at: row.get(2)?,
                count: row.get(3)?,
                finished: row.get::<_, Option<i64>>(4)?.unwrap_or(0),
            })
        })
        .map_err(|e| format!("查询待看目录失败: {e}"))?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取待看目录失败: {e}"))
}

fn queue_by_name(conn: &Connection, name: &str) -> Result<Option<crate::models::Queue>, String> {
    let queues = list_queues(conn)?;
    Ok(queues
        .into_iter()
        .find(|queue| queue.name.eq_ignore_ascii_case(name)))
}

pub fn delete_queue(conn: &Connection, queue_id: i64) -> Result<(), String> {
    let changed = conn
        .execute("DELETE FROM queues WHERE id = ?1", [queue_id])
        .map_err(|e| format!("删除待看目录失败: {e}"))?;

    if changed == 0 {
        return Err("待看目录不存在".to_string());
    }
    Ok(())
}

/// 取待看目录里的成员，按加入顺序。
pub fn queue_archives(
    conn: &Connection,
    queue_id: i64,
) -> Result<Vec<i64>, String> {
    let mut statement = conn
        .prepare(
            "SELECT qa.archive_id
             FROM queue_archives qa
             JOIN archives a ON a.id = qa.archive_id AND a.deleted_at IS NULL
             WHERE qa.queue_id = ?1
             ORDER BY qa.added_at ASC, qa.archive_id ASC",
        )
        .map_err(|e| format!("准备目录成员查询失败: {e}"))?;

    let rows = statement
        .query_map([queue_id], |row| row.get::<_, i64>(0))
        .map_err(|e| format!("查询目录成员失败: {e}"))?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取目录成员失败: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(id: i64, read_count: i64, weight: i64) -> Candidate {
        Candidate {
            id,
            title: format!("archive-{id}"),
            read_count,
            weight,
        }
    }

    #[test]
    fn weighted_pick_never_repeats() {
        let pool = vec![
            candidate(1, 0, 100),
            candidate(2, 0, 100),
            candidate(3, 0, 100),
        ];
        let picked = pick_weighted(pool, 3, 42);
        assert_eq!(picked.len(), 3);
        let mut ids: Vec<i64> = picked.iter().map(|c| c.id).collect();
        ids.sort_unstable();
        assert_eq!(ids, vec![1, 2, 3]);
    }

    #[test]
    fn weighted_pick_stops_when_pool_is_smaller() {
        let pool = vec![candidate(1, 0, 100), candidate(2, 0, 100)];
        let picked = pick_weighted(pool, 10, 7);
        assert_eq!(picked.len(), 2);
    }

    /// 看得少的应该显著更容易被挑中：反复抽样统计出现次数。
    #[test]
    fn read_count_deprioritises_seen_archives() {
        let pool = vec![candidate(1, 0, 100), candidate(2, 9, 100)];
        let mut fresh = 0;
        let mut seen = 0;
        for seed in 0..400u64 {
            let picked = pick_weighted(pool.clone(), 1, seed);
            if picked[0].id == 1 {
                fresh += 1;
            } else {
                seen += 1;
            }
        }
        // 新本权重 100 vs 已看 9 遍的 10，期望约 10:1。
        assert!(fresh > seen * 3, "fresh={fresh} seen={seen}");
    }

    #[test]
    fn sampling_weight_drops_with_reads() {
        assert!(sampling_weight(&candidate(1, 0, 100)) > sampling_weight(&candidate(1, 4, 100)));
        assert_eq!(sampling_weight(&candidate(1, 0, 0)), 0.0);
    }
}
