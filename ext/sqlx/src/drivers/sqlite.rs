use super::{QueryResult, SqliteDriver};
use once_cell::sync::Lazy;
use serde_json::Value;
use sqlx::sqlite::{Sqlite, SqliteArguments, SqlitePoolOptions};
use sqlx::{Column, Pool, Row};

static RT: Lazy<tokio::runtime::Runtime> = Lazy::new(|| {
    tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .expect("sqlx sqlite driver: runtime")
});

pub struct Driver;

impl SqliteDriver for Driver {
    type Pool = Pool<Sqlite>;

    fn connect(url: &str) -> Result<Self::Pool, sqlx::Error> {
        // Normalize simple path or :memory:
        let url = if url.starts_with("sqlite:") {
            url.to_string()
        } else if url == ":memory:" {
            "sqlite::memory:".to_string()
        } else {
            format!("sqlite:{}", url)
        };
        RT.block_on(async move {
            SqlitePoolOptions::new()
                .max_connections(1)
                .connect(url.as_str())
                .await
        })
    }

    fn query(pool: &Self::Pool, sql: &str, params: &[Value]) -> Result<QueryResult, sqlx::Error> {
        let pool = pool.clone();
        let sql = sql.to_string();
        let params = params.to_owned();
        RT.block_on(async move {
            let mut q = sqlx::query(sql.as_str());
            for v in &params {
                q = bind_sqlite(q, v);
            }
            let rows = q.fetch_all(&pool).await?;
            let columns: Vec<String> = rows
                .get(0)
                .map(|r| r.columns().iter().map(|c| c.name().to_string()).collect())
                .unwrap_or_default();
            let col_len = columns.len();
            let mut out_rows: Vec<Vec<Value>> = Vec::with_capacity(rows.len());
            for r in rows.iter() {
                let mut row_vals: Vec<Value> = Vec::with_capacity(col_len);
                for i in 0..col_len {
                    let v = r
                        .try_get::<bool, _>(i)
                        .map(Value::from)
                        .or_else(|_| r.try_get::<i64, _>(i).map(Value::from))
                        .or_else(|_| r.try_get::<f64, _>(i).map(Value::from))
                        .or_else(|_| r.try_get::<String, _>(i).map(Value::from))
                        .unwrap_or(Value::Null);
                    row_vals.push(v);
                }
                out_rows.push(row_vals);
            }
            Ok(QueryResult {
                columns,
                rows: out_rows,
            })
        })
    }
}

fn bind_sqlite<'q>(
    q: sqlx::query::Query<'q, Sqlite, SqliteArguments<'q>>,
    v: &Value,
) -> sqlx::query::Query<'q, Sqlite, SqliteArguments<'q>> {
    match v {
        Value::Null => q.bind::<Option<String>>(None),
        Value::Bool(b) => q.bind(*b as i64),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                q.bind(i)
            } else if let Some(u) = n.as_u64() {
                q.bind(u as i64)
            } else if let Some(f) = n.as_f64() {
                q.bind(f)
            } else {
                q
            }
        }
        Value::String(s) => q.bind(s.clone()),
        other => q.bind(other.to_string()),
    }
}
