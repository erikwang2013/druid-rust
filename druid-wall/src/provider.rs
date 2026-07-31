use std::collections::HashMap;
use crate::checker::{WallChecker, WallCheckResult};

pub struct WallProvider {
    checker: WallChecker, cache: HashMap<String, WallCheckResult>,
    pub hit_count: u64, pub check_count: u64, max_cache_size: usize,
}

impl WallProvider {
    pub fn new(checker: WallChecker, max_cache_size: usize) -> Self {
        WallProvider { checker, cache: HashMap::new(), hit_count: 0, check_count: 0, max_cache_size }
    }

    pub fn check(&mut self, sql: &str) -> WallCheckResult {
        self.check_count += 1;
        if let Some(r) = self.cache.get(sql) { self.hit_count += 1; return r.clone(); }
        let r = self.checker.quick_check(sql);
        if !r.allowed { self.cache_insert(sql, r.clone()); return r; }
        if let Ok(stmts) = druid_sql::parse_sql(sql) {
            for s in &stmts { let x = self.checker.check(sql, s); if !x.allowed { self.cache_insert(sql, x.clone()); return x; } }
        }
        let pass = WallCheckResult::pass(); self.cache_insert(sql, pass.clone()); pass
    }

    fn cache_insert(&mut self, sql: &str, result: WallCheckResult) {
        if self.cache.len() >= self.max_cache_size {
            let keys: Vec<String> = self.cache.keys().take(self.max_cache_size / 2).cloned().collect();
            for k in keys { self.cache.remove(&k); }
        }
        self.cache.insert(sql.into(), result);
    }

    pub fn hit_rate(&self) -> f64 { if self.check_count == 0 { 0.0 } else { self.hit_count as f64 / self.check_count as f64 } }
    pub fn clear_cache(&mut self) { self.cache.clear(); }
    pub fn cache_size(&self) -> usize { self.cache.len() }
}

#[cfg(test)] mod tests { use super::*; use crate::config::WallConfig;
    #[test] fn test_cache() { let c = WallChecker::new(WallConfig::default()); let mut p = WallProvider::new(c, 100); p.check("SELECT 1"); p.check("SELECT 1"); assert_eq!(p.hit_count, 1); } }
