use super::*;

#[test]
fn test_stat_collection() {
    let filter = StatFilter::new("test-ds", 1000);
    let ctx = FilterContext::new("test").with_sql("SELECT 1");

    filter.statement_execute_before(&ctx).unwrap();
    filter.statement_execute_after(&ctx, 50, 1);
    filter.statement_execute_before(&ctx).unwrap();
    filter.statement_execute_after(&ctx, 200, 10);

    let stats = filter.get_sql_stats();
    assert_eq!(stats.len(), 1);
    assert_eq!(stats[0].execute_count, 2);
    assert_eq!(stats[0].max_time_ms, 200);
}

#[test]
fn test_slow_sql_detection() {
    let filter = StatFilter::new("test-ds", 100);
    let ctx = FilterContext::new("test").with_sql("SELECT SLEEP(1)");
    filter.statement_execute_before(&ctx).unwrap();
    filter.statement_execute_after(&ctx, 500, 0);

    let slow = filter.get_slow_sql();
    assert_eq!(slow.len(), 1);
}

#[test]
fn test_datasource_stat() {
    let filter = StatFilter::new("ds1", 1000);
    filter.connection_created(&FilterContext::new("ds1"));
    filter.connection_borrowed(&FilterContext::new("ds1"), 10);

    let stat = filter.get_datasource_stat();
    assert_eq!(stat.create_count, 1);
    assert_eq!(stat.borrow_count, 1);
}

#[test]
fn test_sql_truncated_to_200_chars() {
    let filter = StatFilter::new("ds", 1000);
    let long_sql = "x".repeat(300);
    filter.statement_execute_after(&FilterContext::new("ds").with_sql(&long_sql), 5, 0);
    let stats = filter.get_sql_stats();
    assert_eq!(stats.len(), 1);
    assert!(stats[0].sql.ends_with("..."));
    assert_eq!(stats[0].sql.len(), 200 + "...".len());
}

#[test]
fn test_sql_stats_sorted_by_total_time_desc() {
    let filter = StatFilter::new("ds", 1000);
    filter.statement_execute_after(&FilterContext::new("ds").with_sql("SLOW"), 500, 0);
    filter.statement_execute_after(&FilterContext::new("ds").with_sql("FAST"), 10, 0);
    filter.statement_execute_after(&FilterContext::new("ds").with_sql("SLOW"), 100, 0);

    let stats = filter.get_sql_stats();
    assert_eq!(stats.len(), 2);
    assert_eq!(stats[0].sql, "SLOW"); // 总耗时 600 > 10
    assert_eq!(stats[0].total_time_ms, 600);
    assert_eq!(stats[0].max_time_ms, 500);
    assert_eq!(stats[1].sql, "FAST");
}

#[test]
fn test_rows_read_accumulates() {
    let filter = StatFilter::new("ds", 1000);
    let ctx = FilterContext::new("ds").with_sql("SELECT");
    filter.statement_execute_after(&ctx, 1, 100);
    filter.statement_execute_after(&ctx, 1, 50);
    let stats = filter.get_sql_stats();
    assert_eq!(stats[0].rows_read, 150);
    assert_eq!(stats[0].execute_count, 2);
}

#[test]
fn test_statement_error_counting() {
    let filter = StatFilter::new("ds", 1000);
    let ctx = FilterContext::new("ds").with_sql("BAD SQL");
    let err = DruidError::SqlParse("syntax".into());
    filter.statement_error(&ctx, &err);
    filter.statement_error(&ctx, &err);

    let ds_stat = filter.get_datasource_stat();
    assert_eq!(ds_stat.error_count, 2);
    let stats = filter.get_sql_stats();
    assert_eq!(stats[0].error_count, 2);
}

#[test]
fn test_slow_sql_boundary_inclusive() {
    let filter = StatFilter::new("ds", 100);
    let ctx = FilterContext::new("ds").with_sql("BOUNDARY");
    filter.statement_execute_after(&ctx, 100, 0); // 恰好等于阈值
    assert_eq!(filter.get_slow_sql().len(), 1);

    filter.statement_execute_after(&ctx, 99, 0);
    assert_eq!(filter.get_slow_sql().len(), 1); // 只有一条达到阈值
}

#[test]
fn test_unknown_sql_default_name() {
    let filter = StatFilter::new("ds", 1000);
    filter.statement_execute_after(&FilterContext::new("ds"), 5, 0); // 无 SQL
    let stats = filter.get_sql_stats();
    assert_eq!(stats.len(), 1);
    assert_eq!(stats[0].sql, "UNKNOWN");
}

#[test]
fn test_datasource_stat_full_connection_lifecycle() {
    let filter = StatFilter::new("ds", 1000);
    filter.connection_created(&FilterContext::new("ds"));
    filter.connection_borrowed(&FilterContext::new("ds"), 15);
    filter.connection_returned(&FilterContext::new("ds"));
    filter.connection_closed(&FilterContext::new("ds"));

    let stat = filter.get_datasource_stat();
    assert_eq!(stat.create_count, 1);
    assert_eq!(stat.borrow_count, 1);
    assert_eq!(stat.return_count, 1);
    assert_eq!(stat.destroy_count, 1);
    assert_eq!(stat.total_wait_time_ms, 15);
    // created(+1) → borrowed(-1) → returned(+1) → closed(-1) = 0
    assert_eq!(stat.idle_count, 0);
    assert_eq!(filter.execute_count(), 0);

    filter
        .statement_execute_before(&FilterContext::new("ds"))
        .unwrap();
    assert_eq!(filter.execute_count(), 1);
}

#[test]
fn test_sql_stats_capacity_limit() {
    // ORM 动态 SQL：每个参数值一条不同 SQL，表不能无限增长
    let filter = StatFilter::new("ds", 1000);
    for i in 0..2000 {
        let sql = format!("SELECT * FROM users WHERE id = {i}");
        filter.statement_execute_after(&FilterContext::new("ds").with_sql(&sql), 1, 1);
    }
    assert_eq!(filter.get_sql_stats().len(), DEFAULT_MAX_SQL_SIZE);
}

#[test]
fn test_sql_stats_evicts_lowest_total_time() {
    let filter = StatFilter::new("ds", 1000).with_max_sql_size(2);
    let ctx = |sql: &str| FilterContext::new("ds").with_sql(sql);
    filter.statement_execute_after(&ctx("A"), 100, 0);
    filter.statement_execute_after(&ctx("B"), 50, 0);
    filter.statement_execute_after(&ctx("C"), 10, 0); // 超限，淘汰 B(50)

    let stats = filter.get_sql_stats();
    assert_eq!(stats.len(), 2);
    let sqls: Vec<&str> = stats.iter().map(|s| s.sql.as_str()).collect();
    assert!(sqls.contains(&"A"));
    assert!(sqls.contains(&"C"));
    // 已有 key 的更新不触发淘汰
    filter.statement_execute_after(&ctx("A"), 1, 0);
    assert_eq!(filter.get_sql_stats().len(), 2);
}

#[test]
fn test_sql_stats_max_size_zero_means_unlimited() {
    let filter = StatFilter::new("ds", 1000).with_max_sql_size(0);
    for i in 0..1500 {
        let sql = format!("SELECT {i}");
        filter.statement_execute_after(&FilterContext::new("ds").with_sql(&sql), 1, 0);
    }
    assert_eq!(filter.get_sql_stats().len(), 1500);
}

#[test]
fn test_borrowed_connection_closed_decrements_active() {
    // test_on_borrow 校验失败：连接已借出（active+1）后关闭
    let filter = StatFilter::new("ds", 1000);
    for id in 0..1000u64 {
        let ctx = FilterContext::new("ds").with_connection(id);
        filter.connection_created(&ctx);
        filter.connection_borrowed(&ctx, 1);
        filter.connection_closed(&ctx);
    }
    let stat = filter.get_datasource_stat();
    assert_eq!(stat.active_count, 0);
    assert_eq!(stat.idle_count, 0);
    assert_eq!(stat.destroy_count, 1000);
}

#[test]
fn test_idle_connection_closed_decrements_idle() {
    // 空闲连接被驱逐：只减 idle，不影响 active
    let filter = StatFilter::new("ds", 1000);
    let ctx = FilterContext::new("ds").with_connection(1);
    filter.connection_created(&ctx);
    filter.connection_closed(&ctx);

    let stat = filter.get_datasource_stat();
    assert_eq!(stat.idle_count, 0);
    assert_eq!(stat.active_count, 0);
}

#[test]
fn test_borrow_return_then_close_decrements_idle() {
    // 借出→归还→（空闲期）关闭：归还时已移出借出集合，按空闲处理
    let filter = StatFilter::new("ds", 1000);
    let ctx = FilterContext::new("ds").with_connection(1);
    filter.connection_created(&ctx);
    filter.connection_borrowed(&ctx, 1);
    filter.connection_returned(&ctx);
    filter.connection_closed(&ctx);

    let stat = filter.get_datasource_stat();
    assert_eq!(stat.active_count, 0);
    assert_eq!(stat.idle_count, 0);
}

#[test]
fn test_bound_metrics_are_authoritative() {
    let filter = StatFilter::new("ds", 1000);
    let metrics = Arc::new(PoolMetrics::new());
    metrics.set_active(3);
    metrics.set_idle(5);
    assert!(filter.bind_metrics(metrics.clone()));
    assert!(!filter.bind_metrics(metrics)); // 重复绑定不替换

    // 内部事件计数与池内计数不一致时，以池内为准
    filter.connection_created(&FilterContext::new("ds"));
    filter.connection_borrowed(&FilterContext::new("ds"), 1);
    let stat = filter.get_datasource_stat();
    assert_eq!(stat.active_count, 3);
    assert_eq!(stat.idle_count, 5);
}

#[test]
fn test_get_slow_sql_from_reuses_slice() {
    let filter = StatFilter::new("ds", 100);
    let ctx = |sql: &str| FilterContext::new("ds").with_sql(sql);
    filter.statement_execute_after(&ctx("FAST"), 1, 0);
    filter.statement_execute_after(&ctx("SLOW"), 500, 0);

    let all = filter.get_sql_stats();
    let slow = filter.get_slow_sql_from(&all);
    assert_eq!(all.len(), 2);
    assert_eq!(slow.len(), 1);
    assert_eq!(slow[0].sql, "SLOW");
}
