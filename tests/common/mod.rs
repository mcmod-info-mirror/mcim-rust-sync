//! 集成测试共用的 PostgreSQL 准备工作
//!
//! 每个测试用自己的库，从 `DATABASE_URL` 指向的实例上重建，互不干扰。
//! 表结构由 mcim-rust-api 维护，`fixtures/schema.sql` 是它 migrations 的副本，两边要同步修改。

#![allow(dead_code)]

use sqlx::{AssertSqlSafe, Connection, PgConnection};

const SCHEMA: &str = include_str!("../fixtures/schema.sql");

/// 把连接串里的库名换成 `database`
fn with_database(base: &str, database: &str) -> String {
    let (main, query) = match base.split_once('?') {
        Some((main, query)) => (main, Some(query)),
        None => (base, None),
    };
    let prefix = main
        .rsplit_once('/')
        .map(|(prefix, _)| prefix)
        .unwrap_or(main);
    match query {
        Some(query) => format!("{prefix}/{database}?{query}"),
        None => format!("{prefix}/{database}"),
    }
}

/// 重建名为 `database` 的空库并建好表结构，返回它的连接串
///
/// 没设 `DATABASE_URL` 或连不上就跳过，跳过要出声，否则「测试通过」是假的
pub async fn fresh_database(database: &str) -> Option<String> {
    let Ok(base) = std::env::var("DATABASE_URL") else {
        eprintln!("跳过：没有设置 DATABASE_URL");
        return None;
    };
    let mut admin = match PgConnection::connect(&base).await {
        Ok(conn) => conn,
        Err(e) => {
            eprintln!("跳过：连不上 PostgreSQL（{}）", e);
            return None;
        }
    };
    sqlx::query(AssertSqlSafe(format!(
        "DROP DATABASE IF EXISTS {database} WITH (FORCE)"
    )))
    .execute(&mut admin)
    .await
    .expect("删除测试库失败");
    sqlx::query(AssertSqlSafe(format!("CREATE DATABASE {database}")))
        .execute(&mut admin)
        .await
        .expect("创建测试库失败");

    let url = with_database(&base, database);
    let mut conn = PgConnection::connect(&url).await.expect("连接测试库失败");
    sqlx::raw_sql(SCHEMA)
        .execute(&mut conn)
        .await
        .expect("建表失败");
    Some(url)
}
