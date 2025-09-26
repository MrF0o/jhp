use base64::{Engine as _, engine::general_purpose};
use jhp_extensions::{JhpBuf, JhpCallResult, ok_json, parse_args};
use rusqlite::{
    Connection, Row, Statement, ToSql, params_from_iter,
    types::{Value, ValueRef},
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

thread_local! {
    static CONNS: RefCell<HashMap<u32, Connection>> = RefCell::new(HashMap::new());
    static NEXT_ID: Cell<u32> = Cell::new(1);
}

fn alloc_id() -> u32 {
    NEXT_ID.with(|c| {
        let id = c.get();
        c.set(id.saturating_add(1));
        id
    })
}

fn insert_conn(conn: Connection) -> u32 {
    let id = alloc_id();
    CONNS.with(|m| {
        m.borrow_mut().insert(id, conn);
    });
    id
}

fn json_err<E: std::fmt::Display>(msg: &str, e: E) -> JhpCallResult {
    ok_json(&serde_json::json!({"error": format!("{}: {}", msg, e), "code": 1}))
}

fn err_obj<S: ToString>(msg: S, code: i32) -> JhpCallResult {
    ok_json(&serde_json::json!({"error": msg.to_string(), "code": code}))
}

fn decode_blob(obj: &serde_json::Map<String, serde_json::Value>) -> Option<Vec<u8>> {
    if let Some(serde_json::Value::String(b64)) = obj.get("blob") {
        match general_purpose::STANDARD.decode(b64) {
            Ok(bytes) => Some(bytes),
            Err(_) => None,
        }
    } else {
        None
    }
}

fn value_from_json(v: &serde_json::Value) -> Option<Value> {
    match v {
        serde_json::Value::Null => Some(Value::Null),
        serde_json::Value::Bool(b) => Some(Value::Integer(if *b { 1 } else { 0 })),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(Value::Integer(i))
            } else if let Some(f) = n.as_f64() {
                Some(Value::Real(f))
            } else {
                None
            }
        }
        serde_json::Value::String(s) => Some(Value::Text(s.clone())),
        serde_json::Value::Object(map) => {
            if let Some(bytes) = decode_blob(map) {
                Some(Value::Blob(bytes))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn bind_params<'a>(
    stmt: &mut Statement<'a>,
    params: Option<&serde_json::Value>,
) -> Result<usize, rusqlite::Error> {
    match params {
        None => stmt.execute([]),
        Some(serde_json::Value::Array(arr)) => {
            let vals: Vec<Value> = arr
                .iter()
                .map(|v| value_from_json(v).unwrap_or(Value::Null))
                .collect();
            let refs: Vec<&dyn ToSql> = vals.iter().map(|v| v as &dyn ToSql).collect();
            stmt.execute(params_from_iter(refs))
        }
        Some(serde_json::Value::Object(map)) => {
            let mut vals: Vec<Value> = Vec::new();
            let param_count = stmt.parameter_count();
            for i in 1..=param_count {
                let name_opt = stmt.parameter_name(i);
                if let Some(name) = name_opt {
                    let key = name.trim_start_matches([':', '@', '$', '?']);
                    if let Some(v) = map.get(key).and_then(value_from_json) {
                        vals.push(v);
                    } else {
                        vals.push(Value::Null);
                    }
                } else {
                    vals.push(Value::Null);
                }
            }
            let refs: Vec<&dyn ToSql> = vals.iter().map(|v| v as &dyn ToSql).collect();
            stmt.execute(params_from_iter(refs))
        }
        _ => stmt.execute([]),
    }
}

fn row_to_json(row: &Row) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    for (i, col) in row.as_ref().column_names().iter().enumerate() {
        let val = match row.get_ref_unwrap(i) {
            ValueRef::Null => serde_json::Value::Null,
            ValueRef::Integer(i) => serde_json::json!(i),
            ValueRef::Real(f) => serde_json::json!(f),
            ValueRef::Text(t) => serde_json::Value::String(String::from_utf8_lossy(t).to_string()),
            ValueRef::Blob(b) => {
                let b64 = general_purpose::STANDARD.encode(b);
                let mut m = serde_json::Map::new();
                m.insert("blob".to_string(), serde_json::Value::String(b64));
                m.insert(
                    "length".to_string(),
                    serde_json::Value::Number((b.len() as u64).into()),
                );
                serde_json::Value::Object(m)
            }
        };
        obj.insert((*col).to_string(), val);
    }
    serde_json::Value::Object(obj)
}

extern "C" fn sqlite_test(_buf: JhpBuf) -> JhpCallResult {
    ok_json(&serde_json::json!({"message": "It works!"}))
}

extern "C" fn sqlite_open(buf: JhpBuf) -> JhpCallResult {
    let args = match parse_args(buf) {
        Ok(a) => a,
        Err(_) => return err_obj("invalid args", 1),
    };
    let path = match args.get(0).and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return err_obj("open(path) requires path", 2),
    };
    match Connection::open(path) {
        Ok(conn) => {
            let id = insert_conn(conn);
            ok_json(&serde_json::json!({"db": id}))
        }
        Err(e) => json_err("open failed", e),
    }
}

extern "C" fn sqlite_close(buf: JhpBuf) -> JhpCallResult {
    let args = match parse_args(buf) {
        Ok(a) => a,
        Err(_) => return err_obj("invalid args", 1),
    };
    let id = match args.get(0).and_then(|v| v.as_u64()) {
        Some(n) => n as u32,
        None => return err_obj("close(db) requires handle", 2),
    };
    let removed = CONNS.with(|m| m.borrow_mut().remove(&id));
    if let Some(conn) = removed {
        drop(conn);
        ok_json(&serde_json::json!({"ok": true}))
    } else {
        ok_json(&serde_json::json!({"ok": true}))
    }
}

extern "C" fn sqlite_execute(buf: JhpBuf) -> JhpCallResult {
    let args = match parse_args(buf) {
        Ok(a) => a,
        Err(_) => return err_obj("invalid args", 1),
    };
    let id = match args.get(0).and_then(|v| v.as_u64()) {
        Some(n) => n as u32,
        None => return err_obj("execute(db, sql) missing db", 2),
    };
    let sql = match args.get(1).and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return err_obj("execute(db, sql) missing sql", 2),
    };
    let params = args.get(2);
    let mut out: Option<JhpCallResult> = None;
    CONNS.with(|m| {
        let mut map = m.borrow_mut();
        let Some(conn) = map.get_mut(&id) else {
            out = Some(err_obj("invalid db handle", 3));
            return;
        };
        match conn.prepare(sql) {
            Ok(mut stmt) => match bind_params(&mut stmt, params) {
                Ok(changes) => {
                    let last_id = conn.last_insert_rowid();
                    out = Some(ok_json(
                        &serde_json::json!({"rowsAffected": changes, "lastInsertRowId": last_id}),
                    ));
                }
                Err(e) => {
                    out = Some(json_err("execute failed", e));
                }
            },
            Err(e) => {
                out = Some(json_err("prepare failed", e));
            }
        }
    });
    out.unwrap_or_else(|| err_obj("unknown error", 500))
}

extern "C" fn sqlite_query(buf: JhpBuf) -> JhpCallResult {
    let args = match parse_args(buf) {
        Ok(a) => a,
        Err(_) => return err_obj("invalid args", 1),
    };
    let id = match args.get(0).and_then(|v| v.as_u64()) {
        Some(n) => n as u32,
        None => return err_obj("query(db, sql) missing db", 2),
    };
    let sql = match args.get(1).and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return err_obj("query(db, sql) missing sql", 2),
    };
    let params = args.get(2);
    let limit = args
        .get(3)
        .and_then(|v| v.get("limit"))
        .and_then(|v| v.as_u64())
        .unwrap_or(u64::MAX) as usize;
    let mut out: Option<JhpCallResult> = None;
    CONNS.with(|m| {
        let mut map = m.borrow_mut();
        let Some(conn) = map.get_mut(&id) else {
            out = Some(err_obj("invalid db handle", 3));
            return;
        };
        match conn.prepare(sql) {
            Ok(mut stmt) => {
                let cols: Vec<String> = stmt
                    .column_names()
                    .iter()
                    .map(|c| (*c).to_string())
                    .collect();
                let rows_res = match params {
                    None => stmt.query([]),
                    Some(serde_json::Value::Array(arr)) => {
                        let vals: Vec<Value> = arr
                            .iter()
                            .map(|v| value_from_json(v).unwrap_or(Value::Null))
                            .collect();
                        let refs: Vec<&dyn ToSql> = vals.iter().map(|v| v as &dyn ToSql).collect();
                        stmt.query(params_from_iter(refs))
                    }
                    Some(serde_json::Value::Object(map)) => {
                        let mut vals: Vec<Value> = Vec::new();
                        let param_count = stmt.parameter_count();
                        for i in 1..=param_count {
                            let name_opt = stmt.parameter_name(i);
                            if let Some(name) = name_opt {
                                let key = name.trim_start_matches([':', '@', '$', '?']);
                                if let Some(v) = map.get(key).and_then(value_from_json) {
                                    vals.push(v);
                                } else {
                                    vals.push(Value::Null);
                                }
                            } else {
                                vals.push(Value::Null);
                            }
                        }
                        let refs: Vec<&dyn ToSql> = vals.iter().map(|v| v as &dyn ToSql).collect();
                        stmt.query(params_from_iter(refs))
                    }
                    _ => stmt.query([]),
                };
                match rows_res {
                    Ok(mut rows) => {
                        let mut out_rows: Vec<serde_json::Value> = Vec::new();
                        let mut count = 0usize;
                        while count < limit {
                            match rows.next() {
                                Ok(Some(row)) => {
                                    out_rows.push(row_to_json(&row));
                                    count += 1;
                                }
                                Ok(None) => break,
                                Err(e) => {
                                    out = Some(json_err("row fetch failed", e));
                                    return;
                                }
                            }
                        }
                        out = Some(ok_json(
                            &serde_json::json!({"columns": cols, "rows": out_rows}),
                        ));
                    }
                    Err(e) => {
                        out = Some(json_err("query failed", e));
                    }
                }
            }
            Err(e) => {
                out = Some(json_err("prepare failed", e));
            }
        }
    });
    out.unwrap_or_else(|| err_obj("unknown error", 500))
}

extern "C" fn sqlite_version(_buf: JhpBuf) -> JhpCallResult {
    ok_json(&serde_json::json!({"version": rusqlite::version() }))
}

extern "C" fn sqlite_changes(buf: JhpBuf) -> JhpCallResult {
    let args = match parse_args(buf) {
        Ok(a) => a,
        Err(_) => return err_obj("invalid args", 1),
    };
    let id = match args.get(0).and_then(|v| v.as_u64()) {
        Some(n) => n as u32,
        None => return err_obj("changes(db) missing db", 2),
    };
    let mut out: Option<JhpCallResult> = None;
    CONNS.with(|m| {
        let mut map = m.borrow_mut();
        if let Some(conn) = map.get_mut(&id) {
            out = Some(ok_json(&serde_json::json!({"changes": conn.changes() })));
        } else {
            out = Some(err_obj("invalid db handle", 3));
        }
    });
    out.unwrap()
}

extern "C" fn sqlite_last_insert_rowid(buf: JhpBuf) -> JhpCallResult {
    let args = match parse_args(buf) {
        Ok(a) => a,
        Err(_) => return err_obj("invalid args", 1),
    };
    let id = match args.get(0).and_then(|v| v.as_u64()) {
        Some(n) => n as u32,
        None => return err_obj("last_insert_rowid(db) missing db", 2),
    };
    let mut out: Option<JhpCallResult> = None;
    CONNS.with(|m| {
        let mut map = m.borrow_mut();
        if let Some(conn) = map.get_mut(&id) {
            out = Some(ok_json(
                &serde_json::json!({"id": conn.last_insert_rowid() }),
            ));
        } else {
            out = Some(err_obj("invalid db handle", 3));
        }
    });
    out.unwrap()
}

jhp_extensions::export_jhp_v1! {
    "sqlite_test" => sqlite_test,
    "sqlite_open" => sqlite_open,
    "sqlite_close" => sqlite_close,
    "sqlite_execute" => sqlite_execute,
    "sqlite_query" => sqlite_query,
    "sqlite_version" => sqlite_version,
    "sqlite_changes" => sqlite_changes,
    "sqlite_last_insert_rowid" => sqlite_last_insert_rowid,
}
