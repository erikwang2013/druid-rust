use crate::checker::{WallCheckResult, WallChecker};
use std::collections::{HashMap, VecDeque};

pub struct WallProvider {
    checker: WallChecker,
    cache: HashMap<String, WallCheckResult>,
    order: VecDeque<String>,
    pub hit_count: u64,
    pub check_count: u64,
    max_cache_size: usize,
}

impl WallProvider {
    pub fn new(checker: WallChecker, max_cache_size: usize) -> Self {
        WallProvider {
            checker,
            cache: HashMap::new(),
            order: VecDeque::new(),
            hit_count: 0,
            check_count: 0,
            max_cache_size,
        }
    }

    pub fn check(&mut self, sql: &str) -> WallCheckResult {
        self.check_count += 1;
        if let Some(r) = self.cache.get(sql) {
            self.hit_count += 1;
            return r.clone();
        }
        let result = self.check_uncached(sql);
        self.cache_insert(sql, result.clone());
        result
    }

    /// 未命中缓存时的完整检查：纯文本预检 → 解析 → AST 检查。
    /// fail-closed：任何一步不能通过都拒绝，不再"只告警然后放行"。
    fn check_uncached(&self, sql: &str) -> WallCheckResult {
        // 长度上限与纯文本黑名单前置：解析之前就能拒绝
        let quick = self.checker.quick_check(sql);
        if !quick.allowed {
            return quick;
        }
        match druid_sql::parse_sql(sql) {
            Ok(stmts) => {
                let r = self.checker.check_statement_count(stmts.len());
                if !r.allowed {
                    return r;
                }
                for s in &stmts {
                    let x = self.checker.check(sql, s);
                    if !x.allowed {
                        return x;
                    }
                }
                WallCheckResult::pass()
            }
            Err(e) => {
                let preview: String = sql.chars().take(200).collect();
                tracing::warn!("Wall parse failed for '{}': {}", preview, e);
                self.checker.check_unparsable(sql, &e)
            }
        }
    }

    fn cache_insert(&mut self, sql: &str, result: WallCheckResult) {
        if self.cache.len() >= self.max_cache_size {
            let evict_count = (self.max_cache_size / 2).max(1);
            for _ in 0..evict_count {
                if let Some(old) = self.order.pop_front() {
                    self.cache.remove(&old);
                }
            }
        }
        self.cache.insert(sql.into(), result);
        self.order.push_back(sql.to_string());
    }

    pub fn hit_rate(&self) -> f64 {
        if self.check_count == 0 {
            0.0
        } else {
            self.hit_count as f64 / self.check_count as f64
        }
    }
    pub fn clear_cache(&mut self) {
        self.cache.clear();
        self.order.clear();
    }
    pub fn cache_size(&self) -> usize {
        self.cache.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WallConfig;

    fn provider(max: usize) -> WallProvider {
        WallProvider::new(WallChecker::new(WallConfig::default()), max)
    }

    #[test]
    fn test_cache() {
        let c = WallChecker::new(WallConfig::default());
        let mut p = WallProvider::new(c, 100);
        p.check("SELECT 1");
        p.check("SELECT 1");
        assert_eq!(p.hit_count, 1);
    }

    #[test]
    fn test_check_counts_and_hit_rate() {
        let mut p = provider(100);
        assert_eq!(p.hit_rate(), 0.0); // 空计数不除零
        p.check("SELECT 1");
        p.check("SELECT 1");
        p.check("SELECT 2");
        assert_eq!(p.check_count, 3);
        assert_eq!(p.hit_count, 1);
        assert!((p.hit_rate() - 1.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn test_deny_result_is_cached() {
        let mut p = provider(100);
        let r = p.check("DROP TABLE users");
        assert!(!r.allowed);
        let r = p.check("DROP TABLE users"); // 缓存命中
        assert!(!r.allowed);
        assert_eq!(p.hit_count, 1);
    }

    #[test]
    fn test_parse_failure_is_denied_and_cached() {
        // 无法解析的 SQL：fail-closed 拒绝，且拒绝结果同样进缓存
        let mut p = provider(100);
        let r = p.check("NOT VALID SQL !!!");
        assert!(!r.allowed);
        assert!(r.violations[0].message.contains("unparseable"));
        let r = p.check("NOT VALID SQL !!!"); // 缓存命中
        assert!(!r.allowed);
        assert_eq!(p.hit_count, 1);
    }

    #[test]
    fn test_cache_eviction() {
        let mut p = provider(4);
        for i in 0..6 {
            p.check(&format!("SELECT {}", i));
        }
        // 超过容量时淘汰一半
        assert!(p.cache_size() <= 4);
        assert_eq!(p.order.len(), p.cache_size());
        // 最旧的条目已被淘汰：再次查询应 miss
        let before = p.hit_count;
        p.check("SELECT 0");
        assert_eq!(p.hit_count, before, "最旧条目应已被淘汰");
    }

    #[test]
    fn test_clear_cache() {
        let mut p = provider(100);
        p.check("SELECT 1");
        p.check("SELECT 1");
        p.clear_cache();
        assert_eq!(p.cache_size(), 0);
        assert_eq!(p.hit_count, 1); // 计数保留
        p.check("SELECT 1"); // 清空后重新缓存
        assert_eq!(p.cache_size(), 1);
    }

    #[test]
    fn test_cache_size_never_exceeds_max() {
        let mut p = provider(2);
        for i in 0..10 {
            p.check(&format!("SELECT {}", i));
            assert!(p.cache_size() <= 2);
        }
    }
}
