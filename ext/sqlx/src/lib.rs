#![allow(non_snake_case)]
use jhp_extensions::{JhpBuf, JhpCallResult};
use once_cell::sync::Lazy;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
mod drivers;
use drivers::SqliteDriver as _SqliteDriverTrait;
use drivers::sqlite::Driver as SqliteDriver;

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

struct DbPool(sqlx::Pool<sqlx::Sqlite>);
static POOLS: Lazy<Mutex<HashMap<String, DbPool>>> = Lazy::new(|| Mutex::new(HashMap::new()));

fn ok_json(res: Res) -> JhpCallResult {
    let s = serde_json::to_vec(&res).unwrap_or_default();
    let len = s.len();
    let ptr = Box::into_raw(s.into_boxed_slice()) as *const u8;
    JhpCallResult {
        ok: true,
        data: JhpBuf { ptr, len },
        code: 0,
    }
}

fn parse_args(buf: JhpBuf) -> Result<Vec<Value>, ()> {
    let json = unsafe { std::slice::from_raw_parts(buf.ptr, buf.len) };
    match serde_json::from_slice::<Value>(json) {
        Ok(Value::Array(a)) => Ok(a),
        _ => Err(()),
    }
}

// driver handles binding and shaping; no local bind helpers

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
    let url = if url_raw.starts_with("sqlite:") {
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
    let res = SqliteDriver::connect(&url).map(DbPool);
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
    let chosen = {
        let guard = POOLS.lock().unwrap();
        guard.get(conn_id).map(|p| p.0.clone())
    };
    let pool = match chosen {
        Some(p) => p,
        None => {
            return ok_json(Res::Error {
                message: format!("unknown connection id: {}", conn_id),
            });
        }
    };
    match drivers::sqlite::Driver::query(&pool, sql, &params) {
        Ok(out) => ok_json(Res::QueryResult {
            row_count: out.rows.len(),
            columns: out.columns,
            rows: out.rows,
        }),
        Err(e) => ok_json(Res::Error {
            message: format!("query error: {}", e),
        }),
    }
}

jhp_extensions::export_jhp_v1! {
    "sqlx_connect" => sqlx_connect,
    "sqlx_query" => sqlx_query,
}
