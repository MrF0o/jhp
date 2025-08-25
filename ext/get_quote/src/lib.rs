#![allow(non_snake_case)]

use libc::c_char;
use std::sync::atomic::{AtomicU64, Ordering};

#[repr(C)]
pub struct JhpCFunction {
    pub name: *const c_char,
    pub func: extern "C" fn() -> *const c_char,
}

#[repr(C)]
pub struct JhpRegisterResult {
    pub funcs: *const JhpCFunction,
    pub len: usize,
}

// NUL-terminated static strings for C ABI
static QUOTE_0: &[u8] = b"Talk is cheap. Show me the code. - Linus Torvalds\0";
static QUOTE_1: &[u8] = b"Programs must be written for people to read. - Harold Abelson\0";
static QUOTE_2: &[u8] = b"Simplicity is the soul of efficiency. - Austin Freeman\0";
static QUOTE_3: &[u8] = b"Premature optimization is the root of all evil. - Donald Knuth\0";

extern "C" fn get_quote_impl() -> *const c_char {
    static SEED: AtomicU64 = AtomicU64::new(0x9e3779b97f4a7c15);
    let x = SEED.fetch_add(0x9e3779b97f4a7c15, Ordering::Relaxed);
        // Legacy ABI entrypoint; the engine prefers jhp_register_v1 if present.
        match (x % 4) as usize {
        0 => QUOTE_0.as_ptr() as *const c_char,
        1 => QUOTE_1.as_ptr() as *const c_char,
        2 => QUOTE_2.as_ptr() as *const c_char,
        _ => QUOTE_3.as_ptr() as *const c_char,
    }
}

static NAME_GET_QUOTE: &[u8] = b"get_quote\0";

#[unsafe(no_mangle)]
pub unsafe extern "C" fn jhp_register() -> JhpRegisterResult {
    let boxed: Box<[JhpCFunction; 1]> = Box::new([JhpCFunction {
        name: NAME_GET_QUOTE.as_ptr() as *const c_char,
        func: get_quote_impl,
    }]);
    let ptr = Box::into_raw(boxed) as *const JhpCFunction;
    JhpRegisterResult { funcs: ptr, len: 1 }
}
