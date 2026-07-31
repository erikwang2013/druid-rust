use crate::{Filter, FilterChain};
use druid_core::DruidError;

/// FilterManager — 从配置创建 FilterChain
///
/// 管理 Filter 的注册、初始化和销毁。
pub struct FilterManager;

impl FilterManager {
    /// 从 Filter 列表创建 FilterChain
    pub fn create_chain(data_source_name: &str, filters: Vec<Box<dyn Filter>>) -> FilterChain {
        let mut chain = FilterChain::new(data_source_name);
        for filter in filters {
            chain.add_filter(filter);
        }
        chain
    }

    /// 初始化所有 Filter
    pub fn init_filters(filters: &mut [Box<dyn Filter>]) -> Result<(), DruidError> {
        for filter in filters.iter_mut() {
            filter.init()?;
        }
        Ok(())
    }

    /// 销毁所有 Filter
    pub fn destroy_filters(filters: &mut [Box<dyn Filter>]) {
        for filter in filters.iter_mut() {
            filter.destroy();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FilterAdapter;

    #[test]
    fn test_manager_create_chain() {
        let filters: Vec<Box<dyn Filter>> = vec![
            Box::new(FilterAdapter::new("f1")),
            Box::new(FilterAdapter::new("f2")),
        ];
        let chain = FilterManager::create_chain("ds1", filters);
        assert_eq!(chain.len(), 2);
    }

    #[test]
    fn test_manager_init_destroy() {
        let mut filters: Vec<Box<dyn Filter>> = vec![Box::new(FilterAdapter::new("f1"))];
        assert!(FilterManager::init_filters(&mut filters).is_ok());
        FilterManager::destroy_filters(&mut filters);
    }
}
