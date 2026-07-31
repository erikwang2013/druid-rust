use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum SQLExpr {
    /// 标识符: table.column 或 alias
    Identifier(Vec<String>),
    /// 字符串字面量
    StringLiteral(String),
    /// 数字字面量
    NumberLiteral(String),
    /// NULL
    Null,
    /// 占位符 ?
    Placeholder,
    /// 二元运算: left op right
    BinaryOp {
        left: Box<SQLExpr>,
        op: BinaryOpType,
        right: Box<SQLExpr>,
    },
    /// 一元运算
    UnaryOp { op: UnaryOpType, expr: Box<SQLExpr> },
    /// 函数调用: name(args)
    Function {
        name: String,
        args: Vec<SQLExpr>,
        distinct: bool,
    },
    /// SELECT 子查询
    SubQuery(Box<SQLStatement>),
    /// IN 列表: expr IN (val1, val2, ...)
    InList {
        expr: Box<SQLExpr>,
        list: Vec<SQLExpr>,
        not: bool,
    },
    /// IN 子查询: expr IN (SELECT ...)
    InSubQuery {
        expr: Box<SQLExpr>,
        query: Box<SQLStatement>,
        not: bool,
    },
    /// BETWEEN: expr BETWEEN low AND high
    Between {
        expr: Box<SQLExpr>,
        low: Box<SQLExpr>,
        high: Box<SQLExpr>,
        not: bool,
    },
    /// LIKE
    Like {
        expr: Box<SQLExpr>,
        pattern: Box<SQLExpr>,
        not: bool,
    },
    /// IS NULL / IS NOT NULL
    IsNull { expr: Box<SQLExpr>, not: bool },
    /// EXISTS (SELECT ...)
    Exists(Box<SQLStatement>, bool),
    /// CASE WHEN
    Case {
        expr: Option<Box<SQLExpr>>,
        whens: Vec<(SQLExpr, SQLExpr)>,
        else_expr: Option<Box<SQLExpr>>,
    },
    /// 括号包裹的表达式
    Nested(Box<SQLExpr>),
    /// SELECT 列项中的通配符 *
    Wildcard,
    /// CAST(expr AS type)
    Cast {
        expr: Box<SQLExpr>,
        data_type: String,
    },
    /// ROW_NUMBER() 等窗口函数
    WindowFunction {
        function: Box<SQLExpr>,
        partition_by: Vec<SQLExpr>,
        order_by: Vec<OrderByExpr>,
    },
    /// 聚合函数: COUNT(*)
    Aggregate { name: String, expr: Box<SQLExpr> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOpType {
    Eq,
    Neq,
    Lt,
    Gt,
    Leq,
    Geq,
    And,
    Or,
    Plus,
    Minus,
    Mul,
    Div,
    Mod,
    Concat, // ||
    Like,
    Regex,
}

impl fmt::Display for BinaryOpType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BinaryOpType::Eq => write!(f, "="),
            BinaryOpType::Neq => write!(f, "<>"),
            BinaryOpType::Lt => write!(f, "<"),
            BinaryOpType::Gt => write!(f, ">"),
            BinaryOpType::Leq => write!(f, "<="),
            BinaryOpType::Geq => write!(f, ">="),
            BinaryOpType::And => write!(f, "AND"),
            BinaryOpType::Or => write!(f, "OR"),
            BinaryOpType::Plus => write!(f, "+"),
            BinaryOpType::Minus => write!(f, "-"),
            BinaryOpType::Mul => write!(f, "*"),
            BinaryOpType::Div => write!(f, "/"),
            BinaryOpType::Mod => write!(f, "%"),
            BinaryOpType::Concat => write!(f, "||"),
            BinaryOpType::Like => write!(f, "LIKE"),
            BinaryOpType::Regex => write!(f, "REGEXP"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOpType {
    Not,
    Neg,
    Plus,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OrderByExpr {
    pub expr: SQLExpr,
    pub asc: bool,
}

/// SQL 语句枚举 — 表示所有 DML/DDL 语句类型
#[derive(Debug, Clone, PartialEq)]
pub enum SQLStatement {
    Select(Box<SelectStatement>),
    Insert(InsertStatement),
    Update(UpdateStatement),
    Delete(DeleteStatement),
    CreateTable(CreateTableStatement),
    DropObject(DropStatement),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SelectStatement {
    pub distinct: bool,
    pub columns: Vec<SelectItem>,
    pub from: Option<TableReference>,
    pub joins: Vec<JoinClause>,
    pub where_clause: Option<SQLExpr>,
    pub group_by: Vec<SQLExpr>,
    pub having: Option<SQLExpr>,
    pub order_by: Vec<OrderByExpr>,
    pub limit: Option<SQLExpr>,
    pub offset: Option<SQLExpr>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SelectItem {
    /// 表达式 + 可选别名
    Expr(SQLExpr, Option<String>),
    /// SELECT *
    Wildcard(Option<String>), // table.*
}

#[derive(Debug, Clone, PartialEq)]
pub enum TableReference {
    /// 简单表名
    Table {
        name: String,
        alias: Option<String>,
        schema: Option<String>,
    },
    /// 子查询
    SubQuery(Box<SQLStatement>, String), // 子查询必须有别名
    /// JOIN 表达式（在 FROM 后直接写连接的场景）
    Join(Box<JoinClause>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum JoinType {
    Inner,
    Left,
    Right,
    Full,
    Cross,
}

impl fmt::Display for JoinType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JoinType::Inner => write!(f, "INNER JOIN"),
            JoinType::Left => write!(f, "LEFT JOIN"),
            JoinType::Right => write!(f, "RIGHT JOIN"),
            JoinType::Full => write!(f, "FULL JOIN"),
            JoinType::Cross => write!(f, "CROSS JOIN"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct JoinClause {
    pub join_type: JoinType,
    pub table: TableReference,
    pub on: SQLExpr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InsertStatement {
    pub table: String,
    pub columns: Vec<String>,
    pub values: Vec<Vec<SQLExpr>>,
    pub is_replace: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateStatement {
    pub table: String,
    pub sets: Vec<(String, SQLExpr)>,
    pub where_clause: Option<SQLExpr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeleteStatement {
    pub table: String,
    pub where_clause: Option<SQLExpr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateTableStatement {
    pub if_not_exists: bool,
    pub table: String,
    pub columns: Vec<ColumnDef>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ColumnDef {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub default_value: Option<SQLExpr>,
    pub is_primary_key: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DropObjectType {
    Table,
    View,
    Index,
    Database,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DropStatement {
    pub object_type: DropObjectType,
    pub if_exists: bool,
    pub name: String,
}
