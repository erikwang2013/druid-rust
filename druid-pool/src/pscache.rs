use std::collections::HashMap;

/// PreparedStatement 缓存
pub struct PSCache {
    cache: HashMap<String, PSEntry>,
    max_size: usize,
}

#[derive(Debug)]
struct PSEntry {
    #[allow(dead_code)]
    sql: String,
    hit_count: u64,
}

impl PSCache {
    pub fn new(max_size: usize) -> Self {
        PSCache {
            cache: HashMap::new(),
            max_size,
        }
    }

    /// 缓存查询命中
    pub fn get(&mut self, sql: &str) -> bool {
        if let Some(entry) = self.cache.get_mut(sql) {
            entry.hit_count += 1;
            true
        } else {
            false
        }
    }

    /// 缓存新 SQL
    pub fn put(&mut self, sql: &str) {
        if self.max_size == 0 {
            return; // 缓存已禁用（pool_prepared_statements = false 时 max_size 为 0）
        }
        if self.cache.contains_key(sql) {
            return;
        }
        if self.cache.len() >= self.max_size {
            // 简单淘汰：删除 hit_count 最小的条目
            if let Some(key) = self
                .cache
                .iter()
                .min_by_key(|(_, v)| v.hit_count)
                .map(|(k, _)| k.clone())
            {
                self.cache.remove(&key);
            }
        }
        self.cache.insert(
            sql.to_string(),
            PSEntry {
                sql: sql.to_string(),
                hit_count: 1,
            },
        );
    }

    pub fn len(&self) -> usize {
        self.cache.len()
    }
    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }
    pub fn clear(&mut self) {
        self.cache.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pscache_get_put() {
        let mut cache = PSCache::new(3);
        assert!(!cache.get("SELECT 1"));
        cache.put("SELECT 1");
        assert!(cache.get("SELECT 1"));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn test_pscache_eviction() {
        let mut cache = PSCache::new(2);
        cache.put("A");
        cache.put("B");
        cache.get("A"); // A gets hit, so B has lower count
        cache.put("C"); // should evict B
        assert!(cache.get("A"));
        assert!(cache.get("C"));
        assert!(!cache.get("B"));
    }

    #[test]
    fn test_pscache_zero_max_size_disabled() {
        let mut cache = PSCache::new(0);
        cache.put("A"); // put 应为 no-op
        assert!(cache.is_empty());
        assert!(!cache.get("A"));
    }

    #[test]
    fn test_pscache_duplicate_put_keeps_hit_count() {
        let mut cache = PSCache::new(2);
        cache.put("A");
        assert!(cache.get("A")); // A hit_count = 2
        cache.put("A"); // 重复 put 不重置 hit_count
        cache.put("B");
        cache.put("C"); // 应淘汰 hit_count 最小的 B
        assert!(cache.get("A"));
        assert!(cache.get("C"));
        assert!(!cache.get("B"));
    }

    #[test]
    fn test_pscache_clear() {
        let mut cache = PSCache::new(2);
        cache.put("A");
        cache.put("B");
        cache.clear();
        assert!(cache.is_empty());
        assert!(!cache.get("A"));
    }
}
