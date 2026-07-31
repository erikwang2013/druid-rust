pub mod checker;
pub mod config;
pub mod provider;

use druid_core::DruidError;
use druid_filter::{Filter, FilterContext};
use std::sync::Mutex;

pub use checker::WallChecker;
pub use config::{DenyOperation, WallConfig};
pub use provider::WallProvider;

pub struct WallFilter {
    provider: Mutex<WallProvider>,
}

impl WallFilter {
    pub fn new(config: WallConfig) -> Self {
        let checker = WallChecker::new(config);
        WallFilter {
            provider: Mutex::new(WallProvider::new(checker, 512)),
        }
    }
    pub fn hit_rate(&self) -> f64 {
        self.provider.lock().expect("wall lock poisoned").hit_rate()
    }
}

impl Filter for WallFilter {
    fn name(&self) -> &'static str {
        "wall"
    }

    fn init(&mut self) -> Result<(), DruidError> {
        tracing::info!("WallFilter initialized");
        Ok(())
    }

    fn statement_execute_before(&self, ctx: &FilterContext) -> Result<(), DruidError> {
        if let Some(ref sql) = ctx.sql {
            let result = self.provider.lock().expect("wall lock poisoned").check(sql);
            if !result.allowed {
                let msg = result
                    .violations
                    .first()
                    .map(|v| v.message.clone())
                    .unwrap_or("SQL denied".into());
                return Err(DruidError::Wall(msg));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_allow() {
        let f = WallFilter::new(WallConfig::default());
        let c = FilterContext::new("t").with_sql("SELECT id FROM users WHERE id=1");
        assert!(f.statement_execute_before(&c).is_ok());
    }
    #[test]
    fn test_deny() {
        let f = WallFilter::new(WallConfig::default());
        let c = FilterContext::new("t").with_sql("SELECT SLEEP(10)");
        assert!(matches!(
            f.statement_execute_before(&c),
            Err(DruidError::Wall(_))
        ));
    }
}
