//! AST 规则遍历：函数黑名单 + schema 黑名单。
//! 对一条语句只遍历一次，命中最先返回。
use crate::checker::WallCheckResult;
use druid_sql::ast::{SQLExpr, SQLStatement, SelectItem, TableReference};

struct Ctx<'a> {
    funcs: &'a [String],
    schemas: &'a [String],
}

/// 函数名匹配（与大小写无关）
fn deny_fn(name: &str, deny: &[String]) -> Option<WallCheckResult> {
    let upper = name.to_uppercase();
    if deny.iter().any(|f| f.to_uppercase() == upper) {
        return Some(WallCheckResult::deny(format!(
            "forbidden function: {}",
            name
        )));
    }
    None
}

fn visit(expr: &SQLExpr, ctx: &Ctx) -> Option<WallCheckResult> {
    match expr {
        // 判完函数名后必须继续递归参数，否则 COALESCE(SLEEP(5),0) 这类嵌套会漏网
        SQLExpr::Function { name, args, .. } => {
            if let Some(r) = deny_fn(name, ctx.funcs) {
                return Some(r);
            }
            for a in args {
                if let Some(r) = visit(a, ctx) {
                    return Some(r);
                }
            }
        }
        SQLExpr::Aggregate { name, expr } => {
            if let Some(r) = deny_fn(name, ctx.funcs) {
                return Some(r);
            }
            if let Some(r) = visit(expr, ctx) {
                return Some(r);
            }
        }
        SQLExpr::BinaryOp { left, right, .. } => {
            if let Some(r) = visit(left, ctx) {
                return Some(r);
            }
            if let Some(r) = visit(right, ctx) {
                return Some(r);
            }
        }
        SQLExpr::UnaryOp { expr, .. }
        | SQLExpr::Nested(expr)
        | SQLExpr::IsNull { expr, .. }
        | SQLExpr::Cast { expr, .. } => {
            if let Some(r) = visit(expr, ctx) {
                return Some(r);
            }
        }
        SQLExpr::InList { expr, list, .. } => {
            if let Some(r) = visit(expr, ctx) {
                return Some(r);
            }
            for item in list {
                if let Some(r) = visit(item, ctx) {
                    return Some(r);
                }
            }
        }
        SQLExpr::Between {
            expr, low, high, ..
        } => {
            if let Some(r) = visit(expr, ctx) {
                return Some(r);
            }
            if let Some(r) = visit(low, ctx) {
                return Some(r);
            }
            if let Some(r) = visit(high, ctx) {
                return Some(r);
            }
        }
        SQLExpr::Like { expr, pattern, .. } => {
            if let Some(r) = visit(expr, ctx) {
                return Some(r);
            }
            if let Some(r) = visit(pattern, ctx) {
                return Some(r);
            }
        }
        SQLExpr::Case {
            expr: case_expr,
            whens,
            else_expr,
        } => {
            if let Some(e) = case_expr {
                if let Some(r) = visit(e, ctx) {
                    return Some(r);
                }
            }
            for (cond, result) in whens {
                if let Some(r) = visit(cond, ctx) {
                    return Some(r);
                }
                if let Some(r) = visit(result, ctx) {
                    return Some(r);
                }
            }
            if let Some(e) = else_expr {
                if let Some(r) = visit(e, ctx) {
                    return Some(r);
                }
            }
        }
        SQLExpr::SubQuery(s) | SQLExpr::Exists(s, _) | SQLExpr::InSubQuery { query: s, .. } => {
            return visit_stmt(s, ctx);
        }
        SQLExpr::WindowFunction {
            function,
            partition_by,
            order_by,
        } => {
            if let Some(r) = visit(function, ctx) {
                return Some(r);
            }
            for p in partition_by {
                if let Some(r) = visit(p, ctx) {
                    return Some(r);
                }
            }
            for o in order_by {
                if let Some(r) = visit(&o.expr, ctx) {
                    return Some(r);
                }
            }
        }
        _ => {}
    }
    None
}

/// 表引用：schema 黑名单判定 + 子查询继续下钻
fn visit_table_ref(t: &TableReference, ctx: &Ctx) -> Option<WallCheckResult> {
    match t {
        TableReference::Table {
            schema: Some(schema),
            ..
        } => {
            if ctx.schemas.iter().any(|d| d.eq_ignore_ascii_case(schema)) {
                return Some(WallCheckResult::deny(format!(
                    "forbidden schema: {}",
                    schema
                )));
            }
            None
        }
        TableReference::SubQuery(stmt, _) => visit_stmt(stmt, ctx),
        TableReference::Join(j) => visit_table_ref(&j.table, ctx),
        _ => None,
    }
}

fn visit_stmt(stmt: &SQLStatement, ctx: &Ctx) -> Option<WallCheckResult> {
    match stmt {
        SQLStatement::Select(s) => {
            for cte in &s.with_cte {
                if let Some(r) = visit_stmt(&SQLStatement::Select(cte.query.clone()), ctx) {
                    return Some(r);
                }
            }
            for item in &s.columns {
                if let SelectItem::Expr(e, _) = item {
                    if let Some(r) = visit(e, ctx) {
                        return Some(r);
                    }
                }
            }
            if let Some(ref f) = s.from {
                if let Some(r) = visit_table_ref(f, ctx) {
                    return Some(r);
                }
            }
            for join in &s.joins {
                if let Some(r) = visit(&join.on, ctx) {
                    return Some(r);
                }
                if let Some(r) = visit_table_ref(&join.table, ctx) {
                    return Some(r);
                }
            }
            if let Some(ref w) = s.where_clause {
                if let Some(r) = visit(w, ctx) {
                    return Some(r);
                }
            }
            for e in &s.group_by {
                if let Some(r) = visit(e, ctx) {
                    return Some(r);
                }
            }
            if let Some(ref h) = s.having {
                if let Some(r) = visit(h, ctx) {
                    return Some(r);
                }
            }
            for o in &s.order_by {
                if let Some(r) = visit(&o.expr, ctx) {
                    return Some(r);
                }
            }
            if let Some(ref l) = s.limit {
                if let Some(r) = visit(l, ctx) {
                    return Some(r);
                }
            }
            if let Some(ref off) = s.offset {
                if let Some(r) = visit(off, ctx) {
                    return Some(r);
                }
            }
        }
        SQLStatement::Insert(s) => {
            for row in &s.values {
                for val in row {
                    if let Some(r) = visit(val, ctx) {
                        return Some(r);
                    }
                }
            }
        }
        SQLStatement::Update(s) => {
            for (_, val) in &s.sets {
                if let Some(r) = visit(val, ctx) {
                    return Some(r);
                }
            }
            if let Some(ref w) = s.where_clause {
                if let Some(r) = visit(w, ctx) {
                    return Some(r);
                }
            }
        }
        SQLStatement::Delete(s) => {
            if let Some(ref w) = s.where_clause {
                if let Some(r) = visit(w, ctx) {
                    return Some(r);
                }
            }
        }
        SQLStatement::CreateTable(s) => {
            // 列默认值表达式：MySQL/MariaDB 实测允许 RAND()/USER() 等非确定性函数
            // （INSERT 时求值），必须纳入函数黑名单检查
            for col in &s.columns {
                if let Some(ref d) = col.default_value {
                    if let Some(r) = visit(d, ctx) {
                        return Some(r);
                    }
                }
            }
        }
        _ => {}
    }
    None
}

/// 函数黑名单 + schema 黑名单：对整条语句做一次递归遍历
pub(crate) fn check_expr_rules(
    stmt: &SQLStatement,
    funcs: &[String],
    schemas: &[String],
) -> Option<WallCheckResult> {
    let ctx = Ctx { funcs, schemas };
    visit_stmt(stmt, &ctx)
}
