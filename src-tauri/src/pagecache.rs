//! 进程内字节缓存（LRU，按总字节数封顶）。
//!
//! 阅读器不再把整页图片转成 base64 塞进 IPC：单页 32MB 的图会变成 43MB 的
//! JSON 字符串，Rust 端编码一次、JS 端解码一次，还要在 JS 堆里常驻十几份。
//! 现在页面字节留在后端，前端只拿一个 `gallery://` URL，由自定义协议直供。
//!
//! 缓存只用「已解码/已转码的成品字节」，键里带文件大小与修改时间，
//! 压缩包被替换后旧键自然失效，不需要额外的失效逻辑。

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

/// 一条缓存项。`bytes` 用 `Arc` 包住，取用时只加引用计数，不复制大块内存。
#[derive(Clone)]
pub struct Cached {
    pub name: String,
    pub mime: &'static str,
    pub bytes: Arc<Vec<u8>>,
}

impl Cached {
    pub fn new(name: String, mime: &'static str, bytes: Vec<u8>) -> Self {
        Self {
            name,
            mime,
            bytes: Arc::new(bytes),
        }
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }
}

struct Slot {
    entry: Cached,
    size: usize,
}

pub struct ByteCache {
    budget: usize,
    used: usize,
    /// 最久未使用的键在最前面。
    order: VecDeque<String>,
    slots: HashMap<String, Slot>,
}

impl ByteCache {
    pub fn new(budget: usize) -> Self {
        Self {
            budget,
            used: 0,
            order: VecDeque::new(),
            slots: HashMap::new(),
        }
    }

    #[cfg(test)]
    pub fn used(&self) -> usize {
        self.used
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn get(&mut self, key: &str) -> Option<Cached> {
        let entry = self.slots.get(key)?.entry.clone();
        if let Some(position) = self.order.iter().position(|item| item == key)
            && let Some(moved) = self.order.remove(position)
        {
            self.order.push_back(moved);
        }
        Some(entry)
    }

    pub fn insert(&mut self, key: String, entry: Cached) {
        let size = entry.len();
        // 单条就超过整块预算时直接放弃缓存，否则会把其他条目全部挤掉。
        if size > self.budget {
            return;
        }

        if let Some(previous) = self.slots.remove(&key) {
            self.used = self.used.saturating_sub(previous.size);
            if let Some(position) = self.order.iter().position(|item| item == &key) {
                self.order.remove(position);
            }
        }

        self.used += size;
        self.slots.insert(key.clone(), Slot { entry, size });
        self.order.push_back(key);
        self.evict();
    }

    fn evict(&mut self) {
        while self.used > self.budget {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(slot) = self.slots.remove(&oldest) {
                self.used = self.used.saturating_sub(slot.size);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(size: usize) -> Cached {
        Cached::new("page".to_string(), "image/png", vec![7; size])
    }

    #[test]
    fn evicts_least_recently_used_first() {
        let mut cache = ByteCache::new(100);
        cache.insert("a".to_string(), entry(40));
        cache.insert("b".to_string(), entry(40));
        // 访问 a 之后，b 变成最久未使用。
        assert!(cache.get("a").is_some());
        cache.insert("c".to_string(), entry(40));

        assert!(cache.get("a").is_some());
        assert!(cache.get("b").is_none());
        assert!(cache.get("c").is_some());
        assert!(cache.used() <= 100);
    }

    #[test]
    fn oversized_entry_is_not_cached() {
        let mut cache = ByteCache::new(64);
        cache.insert("huge".to_string(), entry(65));
        assert!(cache.get("huge").is_none());
        assert_eq!(cache.used(), 0);
    }

    #[test]
    fn replacing_same_key_does_not_double_count() {
        let mut cache = ByteCache::new(100);
        cache.insert("a".to_string(), entry(30));
        cache.insert("a".to_string(), entry(10));
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.used(), 10);
        assert_eq!(cache.get("a").unwrap().len(), 10);
    }
}
