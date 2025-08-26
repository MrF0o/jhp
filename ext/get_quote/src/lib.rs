#![allow(non_snake_case)]

use jhp_extensions::{JhpBuf, JhpCallResult, err_message, ok_json};
use std::sync::atomic::{AtomicU64, Ordering};

static QUOTES: &[&str] = &[
    "Talk is cheap. Show me the code. - Linus Torvalds",
    "Programs must be written for people to read. - Harold Abelson",
    "Simplicity is the soul of efficiency. - Austin Freeman",
    "Premature optimization is the root of all evil. - Donald Knuth",
];

extern "C" fn get_quote_v1(_buf: JhpBuf) -> JhpCallResult {
    static SEED: AtomicU64 = AtomicU64::new(0x9e3779b97f4a7c15);
    let x = SEED.fetch_add(0x9e3779b97f4a7c15, Ordering::Relaxed);
    let idx = (x % (QUOTES.len() as u64)) as usize;
    ok_json(&serde_json::json!({ "quote": QUOTES[idx] }))
}

// example of an error returning function
#[allow(dead_code)]
extern "C" fn get_quote_err(_buf: JhpBuf) -> JhpCallResult {
    err_message("not implemented", 1)
}

jhp_extensions::export_jhp_v1! {
    "get_quote" => get_quote_v1,
}
