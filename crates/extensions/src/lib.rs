#![deny(unsafe_op_in_unsafe_fn)]
//! jhp_extensions: helpers for writing JHP native extensions safely.
//! - Provides v1 JSON ABI types shared with the engine
//! - Utilities to return JSON easily and free buffers correctly
//! - Macros to export functions and register tables

pub use libc as __libc;
use libc::c_uchar;
use serde::Serialize;

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

/// Allocate a JSON payload from any Serialize value.
pub fn ok_json<T: Serialize>(val: &T) -> JhpCallResult {
    let bytes = match serde_json::to_vec(val) {
        Ok(b) => b,
        Err(_) => Vec::new(),
    };
    let len = bytes.len();
    let ptr = Box::into_raw(bytes.into_boxed_slice()) as *const c_uchar;
    JhpCallResult {
        ok: true,
        data: JhpBuf { ptr, len },
        code: 0,
    }
}

/// Return an error with optional message JSON object: {"error": message}
pub fn err_message(message: &str, code: i32) -> JhpCallResult {
    #[derive(Serialize)]
    struct Err<'a> {
        error: &'a str,
    }
    let bytes = serde_json::to_vec(&Err { error: message }).unwrap_or_default();
    let len = bytes.len();
    let ptr = Box::into_raw(bytes.into_boxed_slice()) as *const c_uchar;
    JhpCallResult {
        ok: false,
        data: JhpBuf { ptr, len },
        code,
    }
}

/// Free function to release buffers allocated by ok_json/err_message
pub extern "C" fn free_v1(ptr: *const c_uchar, len: usize) {
    if !ptr.is_null() && len > 0 {
        // SAFETY: caller guarantees ptr/len from Box<[u8]> allocation
        unsafe {
            drop(Box::from_raw(std::slice::from_raw_parts_mut(
                ptr as *mut u8,
                len,
            )))
        }
    }
}

/// Parse incoming JhpBuf as a serde_json::Value array.
pub fn parse_args(buf: JhpBuf) -> Result<Vec<serde_json::Value>, ()> {
    let slice = unsafe { std::slice::from_raw_parts(buf.ptr, buf.len) };
    match serde_json::from_slice::<serde_json::Value>(slice) {
        Ok(serde_json::Value::Array(a)) => Ok(a),
        _ => Err(()),
    }
}

/// Create a JhpRegisterV1 from a static list of function descriptors.
pub fn register_v1(funcs: Box<[JhpFunctionDescV1]>) -> JhpRegisterV1 {
    let ptr = Box::into_raw(funcs) as *const JhpFunctionDescV1;
    JhpRegisterV1 {
        abi_version: 1,
        funcs: ptr,
        len: unsafe_count(ptr),
        free_fn: free_v1,
    }
}

#[inline]
fn unsafe_count(ptr: *const JhpFunctionDescV1) -> usize {
    // Caller passes a Box<[...]>; we cannot recover length from pointer safely here.
    // callers should build JhpRegisterV1 manually setting len. Keep function for API symmetry.
    // we return 0 to force callers to not use this in practice inadvertently.
    // not used by macros below which set len explicitly.
    let _ = ptr;
    0
}

/// Create a NUL-terminated static C string from a Rust &'static str.
#[macro_export]
macro_rules! cstr {
    ($s:expr) => {{ concat!($s, "\0").as_ptr() as *const $crate::__libc::c_char }};
}

/// Export a v1 extension registry with the given function table.
/// Usage: export_jhp!(
///   fn_name => extern "C" fn(JhpBuf) -> JhpCallResult,
///   ...
/// )
#[macro_export]
macro_rules! export_jhp_v1 {
    ($($name:expr => $func:path),+ $(,)?) => {
    #[unsafe(no_mangle)]
        pub unsafe extern "C" fn jhp_register_v1() -> $crate::JhpRegisterV1 {
            let boxed: Box<[$crate::JhpFunctionDescV1]> = vec![
                $( $crate::JhpFunctionDescV1 { name: $crate::cstr!($name), call: $func }, )+
            ].into_boxed_slice();
            let len = boxed.len();
            let ptr = Box::into_raw(boxed) as *const $crate::JhpFunctionDescV1;
            $crate::JhpRegisterV1 { abi_version: 1, funcs: ptr, len, free_fn: $crate::free_v1 }
        }
    };
}
