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
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[test]
    fn test_manager_create_chain() {
        let filters: Vec<Box<dyn Filter>> = vec![
            Box::new(FilterAdapter::new("f1")),
            Box::new(FilterAdapter::new("f2")),
        ];
        let chain = FilterManager::create_chain("ds1", filters);
        assert_eq!(chain.len(), 2);
        assert_eq!(chain.filter_names(), vec!["f1", "f2"]);
    }

    #[test]
    fn test_manager_create_empty_chain() {
        let chain = FilterManager::create_chain("ds1", vec![]);
        assert!(chain.is_empty());
        chain.connection_created(1); // 空链调用不 panic
    }

    #[test]
    fn test_manager_init_destroy() {
        let mut filters: Vec<Box<dyn Filter>> = vec![Box::new(FilterAdapter::new("f1"))];
        assert!(FilterManager::init_filters(&mut filters).is_ok());
        FilterManager::destroy_filters(&mut filters);
    }

    /// init 失败的 Filter：记录 init/destroy 调用，可注入失败
    struct LifecycleFilter {
        name: &'static str,
        inited: Arc<AtomicUsize>,
        destroyed: Arc<AtomicUsize>,
        fail_init: bool,
    }

    impl Filter for LifecycleFilter {
        fn name(&self) -> &'static str {
            self.name
        }
        fn init(&mut self) -> Result<(), DruidError> {
            self.inited.fetch_add(1, Ordering::SeqCst);
            if self.fail_init {
                Err(DruidError::Config("init failed".into()))
            } else {
                Ok(())
            }
        }
        fn destroy(&mut self) {
            self.destroyed.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn test_init_filters_all_success() {
        let inited = Arc::new(AtomicUsize::new(0));
        let destroyed = Arc::new(AtomicUsize::new(0));
        let mut filters: Vec<Box<dyn Filter>> = (0..3)
            .map(|_| {
                Box::new(LifecycleFilter {
                    name: "f",
                    inited: inited.clone(),
                    destroyed: destroyed.clone(),
                    fail_init: false,
                }) as Box<dyn Filter>
            })
            .collect();
        assert!(FilterManager::init_filters(&mut filters).is_ok());
        assert_eq!(inited.load(Ordering::SeqCst), 3);
        FilterManager::destroy_filters(&mut filters);
        assert_eq!(destroyed.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn test_init_filters_stops_on_first_error() {
        let inited = Arc::new(AtomicUsize::new(0));
        let mut filters: Vec<Box<dyn Filter>> = vec![
            Box::new(LifecycleFilter {
                name: "ok",
                inited: inited.clone(),
                destroyed: Arc::new(AtomicUsize::new(0)),
                fail_init: false,
            }),
            Box::new(LifecycleFilter {
                name: "bad",
                inited: inited.clone(),
                destroyed: Arc::new(AtomicUsize::new(0)),
                fail_init: true,
            }),
            Box::new(LifecycleFilter {
                name: "never",
                inited: inited.clone(),
                destroyed: Arc::new(AtomicUsize::new(0)),
                fail_init: false,
            }),
        ];
        let err = FilterManager::init_filters(&mut filters);
        assert!(matches!(err, Err(DruidError::Config(_))));
        // 前两个被调用（第二个失败），第三个不应被 init
        assert_eq!(inited.load(Ordering::SeqCst), 2);
    }
}
