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
