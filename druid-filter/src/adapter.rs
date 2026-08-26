use crate::Filter;

/// FilterAdapter — Filter trait 的默认空实现
///
/// 继承此结构体并覆写所需方法，避免实现所有 Filter 方法。
/// 对应 Java 的 FilterAdapter。
pub struct FilterAdapter {
    name: &'static str,
}

impl FilterAdapter {
    pub fn new(name: &'static str) -> Self {
        FilterAdapter { name }
    }
}

impl Filter for FilterAdapter {
    fn name(&self) -> &'static str {
        self.name
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FilterContext, Filter};

    #[test]
    fn test_adapter_name() {
        let a = FilterAdapter::new("my_filter");
        assert_eq!(a.name(), "my_filter");
    }

    #[test]
    fn test_adapter_default_methods_are_noop() {
        let a = FilterAdapter::new("f");
        let ctx = FilterContext::new("ds");
        assert!(a.statement_execute_before(&ctx).is_ok());
        a.connection_created(&ctx);
        a.resultset_closed(&ctx, 10);
        // 空实现不 panic、不改状态即可
    }

    #[test]
    fn test_adapter_as_boxed_filter() {
        let f: Box<dyn Filter> = Box::new(FilterAdapter::new("boxed"));
        assert_eq!(f.name(), "boxed");
    }
}
