#![allow(non_snake_case)]
use libc::c_uchar;
use once_cell::sync::Lazy;
use serde::Serialize;
use serde_json::Value;
use sqlx::mysql::{MySql, MySqlArguments, MySqlPoolOptions};
use sqlx::postgres::{PgArguments, PgPoolOptions, Postgres};
use sqlx::sqlite::{Sqlite, SqliteArguments, SqlitePoolOptions};
use sqlx::{Column, Pool, Row};
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

#[repr(C)]
pub struct JhpBuf {
    pub ptr: *const c_uchar,
    pub len: usize,
}
#[repr(C)]
pub struct JhpCallResult {
    pub ok: bool,
    pub data: JhpBuf,
    pub code: i32,
}

pub type ExtCallV1 = extern "C" fn(JhpBuf) -> JhpCallResult;
pub type ExtFreeV1 = extern "C" fn(*const c_uchar, usize);

#[repr(C)]
pub struct JhpFunctionDescV1 {
    pub name: *const libc::c_char,
    pub call: ExtCallV1,
}
#[repr(C)]
pub struct JhpRegisterV1 {
    pub abi_version: u32,
    pub funcs: *const JhpFunctionDescV1,
    pub len: usize,
    pub free_fn: ExtFreeV1,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Res {
    Connected {
        id: String,
    },
    QueryResult {
        columns: Vec<String>,
        rows: Vec<Vec<Value>>,
        row_count: usize,
    },
    Error {
        message: String,
    },
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static RT: Lazy<tokio::runtime::Runtime> = Lazy::new(|| {
    tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .expect("sqlx ext: runtime")
});

enum DbPool {
    Postgres(Pool<Postgres>),
    MySql(Pool<MySql>),
    Sqlite(Pool<Sqlite>),
}
static POOLS: Lazy<Mutex<HashMap<String, DbPool>>> = Lazy::new(|| Mutex::new(HashMap::new()));

fn ok_json(res: Res) -> JhpCallResult {
    let s = serde_json::to_vec(&res).unwrap_or_default();
    let len = s.len();
    let ptr = Box::into_raw(s.into_boxed_slice()) as *const c_uchar;
    JhpCallResult {
        ok: true,
        data: JhpBuf { ptr, len },
        code: 0,
    }
}

extern "C" fn free_v1(ptr: *const c_uchar, len: usize) {
    if !ptr.is_null() && len > 0 {
        unsafe {
            drop(Box::from_raw(std::slice::from_raw_parts_mut(
                ptr as *mut u8,
                len,
            )))
        }
    }
}

fn parse_args(buf: JhpBuf) -> Result<Vec<Value>, ()> {
    let json = unsafe { std::slice::from_raw_parts(buf.ptr, buf.len) };
    match serde_json::from_slice::<Value>(json) {
        Ok(Value::Array(a)) => Ok(a),
        _ => Err(()),
    }
}

fn bind_pg<'q>(
    q: sqlx::query::Query<'q, Postgres, PgArguments>,
    v: &Value,
) -> sqlx::query::Query<'q, Postgres, PgArguments> {
    match v {
        Value::Null => q.bind::<Option<i64>>(None),
        Value::Bool(b) => q.bind(*b),
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
        other => q.bind(sqlx::types::Json(other.clone())),
    }
}
fn bind_mysql<'q>(
    q: sqlx::query::Query<'q, MySql, MySqlArguments>,
    v: &Value,
) -> sqlx::query::Query<'q, MySql, MySqlArguments> {
    match v {
        Value::Null => q.bind::<Option<i64>>(None),
        Value::Bool(b) => q.bind(*b),
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
        other => q.bind(sqlx::types::Json(other.clone())),
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

extern "C" fn sqlx_connect(buf: JhpBuf) -> JhpCallResult {
    let args = match parse_args(buf) {
        Ok(a) => a,
        Err(_) => {
            return ok_json(Res::Error {
                message: "invalid args for sqlx_connect".into(),
            });
        }
    };
    let url_raw = args.get(0).and_then(|v| v.as_str()).unwrap_or("");
    // Normalize sqlite connection strings: allow plain paths and :memory:
    let url = if url_raw.starts_with("postgres://")
        || url_raw.starts_with("postgresql://")
        || url_raw.starts_with("mysql://")
        || url_raw.starts_with("sqlite:")
    {
        url_raw.to_string()
    } else if url_raw == ":memory:" {
        "sqlite::memory:".to_string()
    } else if url_raw.ends_with(".db") || url_raw.ends_with(".sqlite") || url_raw.contains('/') {
        format!("sqlite:{}", url_raw)
    } else {
        url_raw.to_string()
    };
    if url.is_empty() {
        return ok_json(Res::Error {
            message: "missing database url".into(),
        });
    }
    let id = format!("pool_{}", NEXT_ID.fetch_add(1, Ordering::Relaxed));
    let res = if url.starts_with("postgres://") || url.starts_with("postgresql://") {
        RT.block_on(async {
            PgPoolOptions::new()
                .max_connections(1)
                .connect(url.as_str())
                .await
        })
        .map(DbPool::Postgres)
    } else if url.starts_with("mysql://") {
        RT.block_on(async {
            MySqlPoolOptions::new()
                .max_connections(1)
                .connect(url.as_str())
                .await
        })
        .map(DbPool::MySql)
    } else if url.starts_with("sqlite:") || url.ends_with(".db") || url.ends_with(".sqlite") {
        RT.block_on(async {
            SqlitePoolOptions::new()
                .max_connections(1)
                .connect(url.as_str())
                .await
        })
        .map(DbPool::Sqlite)
    } else {
        return ok_json(Res::Error {
            message: format!("unsupported or unknown database url: {}", url),
        });
    };
    match res {
        Ok(pool) => {
            POOLS.lock().unwrap().insert(id.clone(), pool);
            ok_json(Res::Connected { id })
        }
        Err(e) => ok_json(Res::Error {
            message: format!("connect error: {}", e),
        }),
    }
}

extern "C" fn sqlx_query(buf: JhpBuf) -> JhpCallResult {
    let args = match parse_args(buf) {
        Ok(a) => a,
        Err(_) => {
            return ok_json(Res::Error {
                message: "invalid args for sqlx_query".into(),
            });
        }
    };
    let conn_id = match args.get(0) {
        Some(Value::String(s)) => s.as_str(),
        Some(Value::Object(m)) => m.get("id").and_then(|v| v.as_str()).unwrap_or(""),
        _ => "",
    };
    if conn_id.is_empty() {
        return ok_json(Res::Error {
            message: "missing connection id".into(),
        });
    }
    let sql = args.get(1).and_then(|v| v.as_str()).unwrap_or("");
    let params = args
        .get(2)
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // choose pool clone
    enum Chosen {
        Postgres(Pool<Postgres>),
        MySql(Pool<MySql>),
        Sqlite(Pool<Sqlite>),
    }
    let chosen = {
        let guard = POOLS.lock().unwrap();
        match guard.get(conn_id) {
            Some(DbPool::Postgres(pg)) => Chosen::Postgres(pg.clone()),
            Some(DbPool::MySql(my)) => Chosen::MySql(my.clone()),
            Some(DbPool::Sqlite(sq)) => Chosen::Sqlite(sq.clone()),
            None => {
                return ok_json(Res::Error {
                    message: format!("unknown connection id: {}", conn_id),
                });
            }
        }
    };

    let shaped = match chosen {
        Chosen::Postgres(pg) => RT.block_on(async move {
            let mut q = sqlx::query(sql);
            for p in &params {
                q = bind_pg(q, p);
            }
            let rows = q.fetch_all(&pg).await?;
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
            Ok::<_, sqlx::Error>((columns, out_rows))
        }),
        Chosen::MySql(my) => RT.block_on(async move {
            let mut q = sqlx::query(sql);
            for p in &params {
                q = bind_mysql(q, p);
            }
            let rows = q.fetch_all(&my).await?;
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
            Ok::<_, sqlx::Error>((columns, out_rows))
        }),
        Chosen::Sqlite(sq) => RT.block_on(async move {
            let mut q = sqlx::query(sql);
            for p in &params {
                q = bind_sqlite(q, p);
            }
            let rows = q.fetch_all(&sq).await?;
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
            Ok::<_, sqlx::Error>((columns, out_rows))
        }),
    };
    match shaped {
        Ok((columns, rows)) => ok_json(Res::QueryResult {
            row_count: rows.len(),
            columns,
            rows,
        }),
        Err(e) => ok_json(Res::Error {
            message: format!("query error: {}", e),
        }),
    }
}

static NAME_SQLX_CONNECT: &[u8] = b"sqlx_connect\0";
static NAME_SQLX_QUERY: &[u8] = b"sqlx_query\0";

#[unsafe(no_mangle)]
pub unsafe extern "C" fn jhp_register_v1() -> JhpRegisterV1 {
    let boxed: Box<[JhpFunctionDescV1; 2]> = Box::new([
        JhpFunctionDescV1 {
            name: NAME_SQLX_CONNECT.as_ptr() as *const libc::c_char,
            call: sqlx_connect,
        },
        JhpFunctionDescV1 {
            name: NAME_SQLX_QUERY.as_ptr() as *const libc::c_char,
            call: sqlx_query,
        },
    ]);
    let ptr = Box::into_raw(boxed) as *const JhpFunctionDescV1;
    JhpRegisterV1 {
        abi_version: 1,
        funcs: ptr,
        len: 2,
        free_fn: free_v1,
    }
}
