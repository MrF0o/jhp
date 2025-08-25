use jhp_executor::BindingInstaller;
use libloading::Library;
use std::ffi::{CStr, OsStr};
use std::fs;
use std::os::raw::{c_char, c_uchar};
use std::path::{Path, PathBuf};

/// Legacy C-ABI: functions with no args returning C-style const char*.
#[repr(C)]
pub struct JhpCFunctionLegacy {
    pub name: *const c_char,
    pub func: extern "C" fn() -> *const c_char,
}

#[repr(C)]
pub struct JhpRegisterLegacy {
    pub funcs: *const JhpCFunctionLegacy,
    pub len: usize,
}

pub type ExtRegisterLegacyFn = unsafe extern "C" fn() -> JhpRegisterLegacy;

/// v1 ABI: JSON-in/JSON-out
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
    pub name: *const c_char,
    pub call: ExtCallV1,
}

#[repr(C)]
pub struct JhpRegisterV1 {
    pub abi_version: u32, // must be 1
    pub funcs: *const JhpFunctionDescV1,
    pub len: usize,
    pub free_fn: ExtFreeV1,
}

pub type ExtRegisterV1Fn = unsafe extern "C" fn() -> JhpRegisterV1;

type CFuncLegacy = extern "C" fn() -> *const c_char;

fn make_v8_func_from_c_legacy<'s>(
    scope: &mut v8::ContextScope<'s, v8::HandleScope>,
    func_ptr: CFuncLegacy,
) -> v8::Local<'s, v8::Function> {
    // Store the function pointer value directly in an External (no heap allocation/leak).
    let raw_fn: *const () = func_ptr as *const ();
    let ext = v8::External::new(scope, raw_fn as *mut std::ffi::c_void);

    let cb = |scope: &mut v8::HandleScope,
              args: v8::FunctionCallbackArguments,
              mut rv: v8::ReturnValue| {
        if let Ok(ext) = v8::Local::<v8::External>::try_from(args.data()) {
            let raw = ext.value() as *const ();
            if !raw.is_null() {
                let func: CFuncLegacy = unsafe { std::mem::transmute(raw) };
                let s_ptr = (func)();
                if !s_ptr.is_null() {
                    // SAFETY: extension promises valid NUL-terminated UTF-8
                    let s = unsafe { CStr::from_ptr(s_ptr) }.to_string_lossy();
                    if let Some(v) = v8::String::new(scope, &s) {
                        rv.set(v.into());
                    }
                }
            }
        }
    };

    v8::Function::builder(cb)
        .data(ext.into())
        .build(scope)
        .expect("build ext function")
}

fn make_v8_func_from_c_v1<'s>(
    scope: &mut v8::ContextScope<'s, v8::HandleScope>,
    func_ptr: ExtCallV1,
    free_fn: ExtFreeV1,
) -> v8::Local<'s, v8::Function> {
    // Pack two pointers (call, free) into a pair stored via External array-like layout.
    #[repr(C)]
    struct Pair {
        call: ExtCallV1,
        free_fn: ExtFreeV1,
    }
    let pair = Pair {
        call: func_ptr,
        free_fn,
    };
    let raw = Box::into_raw(Box::new(pair)) as *mut std::ffi::c_void;
    let ext = v8::External::new(scope, raw);

    let cb = |scope: &mut v8::HandleScope,
              args: v8::FunctionCallbackArguments,
              mut rv: v8::ReturnValue| {
        // Marshal args to JSON string
        let arr = v8::Array::new(scope, args.length());
        for i in 0..args.length() {
            let v = args.get(i);
            let _ = arr.set_index(scope, i as u32, v);
        }
        let global = scope.get_current_context().global(scope);
        let json_key = v8::String::new(scope, "JSON").unwrap();
        let json_val = global.get(scope, json_key.into()).unwrap();
        let json_ctor: v8::Local<v8::Object> = json_val.try_into().unwrap();
        let stringify_key = v8::String::new(scope, "stringify").unwrap();
        let stringify_val = json_ctor.get(scope, stringify_key.into()).unwrap();
        let stringify: v8::Local<v8::Function> = stringify_val.try_into().unwrap();
        let undef = v8::undefined(scope).into();
        let js_args = [arr.into()];
        let json_val = stringify.call(scope, undef, &js_args).unwrap();
        let json_str = json_val.to_rust_string_lossy(scope);

        // Retrieve pair
        let pair_ptr = v8::Local::<v8::External>::try_from(args.data())
            .map(|e| e.value() as *mut Pair)
            .unwrap();
        let pair_ref = unsafe { &*pair_ptr };

        let buf = JhpBuf {
            ptr: json_str.as_ptr(),
            len: json_str.len(),
        };
        // Call extension
        let res = (pair_ref.call)(buf);
        if res.ok && !res.data.ptr.is_null() && res.data.len > 0 {
            // SAFETY: extension promises UTF-8 JSON
            let s = unsafe {
                std::str::from_utf8_unchecked(std::slice::from_raw_parts(
                    res.data.ptr,
                    res.data.len,
                ))
            };
            if let Some(json_str) = v8::String::new(scope, s) {
                // JSON.parse to return structured value
                let global = scope.get_current_context().global(scope);
                let json_key = v8::String::new(scope, "JSON").unwrap();
                let json_val = global.get(scope, json_key.into()).unwrap();
                let json_obj: v8::Local<v8::Object> = json_val.try_into().unwrap();
                let parse_key = v8::String::new(scope, "parse").unwrap();
                let parse_val = json_obj.get(scope, parse_key.into()).unwrap();
                let parse_fn: v8::Local<v8::Function> = parse_val.try_into().unwrap();
                let undef = v8::undefined(scope).into();
                let args = [json_str.into()];
                if let Some(parsed) = parse_fn.call(scope, undef, &args) {
                    rv.set(parsed);
                }
            }
        }
        // Free returned buffer if any
        if !res.data.ptr.is_null() && res.data.len > 0 {
            (pair_ref.free_fn)(res.data.ptr, res.data.len);
        }
    };

    v8::Function::builder(cb)
        .data(ext.into())
        .build(scope)
        .expect("build ext v1 function")
}

/// Load all native extensions from `ext_dir`, Returns the combined list
/// of installers to install into each V8 context.
pub fn load_installers(ext_dir: &Path) -> Vec<BindingInstaller> {
    let mut installers: Vec<BindingInstaller> = Vec::new();
    if !ext_dir.exists() {
        return installers;
    }

    // 1) Native extensions (*.so) discovered recursively
    fn collect_sos(dir: &Path, out: &mut Vec<PathBuf>) {
        if let Ok(entries) = fs::read_dir(dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    collect_sos(&p, out);
                } else if p.extension() == Some(OsStr::new("so")) {
                    out.push(p);
                }
            }
        }
    }
    let mut libs: Vec<PathBuf> = Vec::new();
    collect_sos(ext_dir, &mut libs);

    for lib_path in libs {
        unsafe {
            match Library::new(&lib_path) {
                Ok(lib) => {
                    // Safety: leak the lib to keep it alive for the process lifetime
                    let lib = Box::leak(Box::new(lib));
                    // prefer v1 ABI if available
                    if let Ok(sym_v1) = lib.get::<ExtRegisterV1Fn>(b"jhp_register_v1") {
                        let reg = sym_v1();
                        if reg.abi_version == 1 && !reg.funcs.is_null() && reg.len > 0 {
                            let slice = std::slice::from_raw_parts(reg.funcs, reg.len);
                            for fdesc in slice.iter() {
                                if fdesc.name.is_null() {
                                    continue;
                                }
                                let name = match CStr::from_ptr(fdesc.name).to_str() {
                                    Ok(s) => s.to_owned(),
                                    Err(_) => continue,
                                };
                                let call = fdesc.call;
                                let free_fn = reg.free_fn;
                                let installer: BindingInstaller =
                                    std::sync::Arc::new(move |scope| {
                                        let name_v8 = v8::String::new(scope, &name).unwrap();
                                        let func = make_v8_func_from_c_v1(scope, call, free_fn);
                                        let global = scope.get_current_context().global(scope);
                                        let _ = global.set(scope, name_v8.into(), func.into());
                                    });
                                installers.push(installer);
                            }
                        }
                    } else if let Ok(sym_legacy) = lib.get::<ExtRegisterLegacyFn>(b"jhp_register") {
                        let reg_res = sym_legacy();
                        if reg_res.len > 0 && !reg_res.funcs.is_null() {
                            let slice = std::slice::from_raw_parts(reg_res.funcs, reg_res.len);
                            for cfn in slice.iter() {
                                if cfn.name.is_null() || (cfn.func as usize) == 0 {
                                    continue;
                                }
                                let name = match CStr::from_ptr(cfn.name).to_str() {
                                    Ok(s) => s.to_owned(),
                                    Err(_) => continue,
                                };
                                let func_ptr = cfn.func;
                                let installer: BindingInstaller =
                                    std::sync::Arc::new(move |scope| {
                                        let name_v8 = v8::String::new(scope, &name).unwrap();
                                        let func = make_v8_func_from_c_legacy(scope, func_ptr);
                                        let global = scope.get_current_context().global(scope);
                                        let _ = global.set(scope, name_v8.into(), func.into());
                                    });
                                installers.push(installer);
                            }
                        }
                    } else {
                        eprintln!(
                            "extension load: no supported register symbol in {}",
                            lib_path.display()
                        );
                    }
                }
                Err(e) => {
                    eprintln!("failed to load extension {}: {}", lib_path.display(), e);
                }
            }
        }
    }

    installers
}

/// discover js extensions under `ext_dir` recursively and produce installers that run them.
pub fn load_js_installers(ext_dir: &Path) -> Vec<BindingInstaller> {
    let mut installers: Vec<BindingInstaller> = Vec::new();
    if !ext_dir.exists() {
        return installers;
    }

    fn collect_js(dir: &Path, out: &mut Vec<PathBuf>) {
        if let Ok(read) = fs::read_dir(dir) {
            for e in read.flatten() {
                let p = e.path();
                if p.is_dir() {
                    collect_js(&p, out);
                } else if p.extension() == Some(OsStr::new("js")) {
                    out.push(p);
                }
            }
        }
    }
    let mut files = Vec::new();
    collect_js(ext_dir, &mut files);
    // sort by path
    files.sort();

    for path in files {
        let resource = path.display().to_string();
        let code = match fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let installer: BindingInstaller = std::sync::Arc::new(move |scope| {
            // Compile and run this JS in the context
            let _ = jhp_executor::v8utils::compile_and_run_current(scope, &code, &resource);
        });
        installers.push(installer);
    }
    installers
}
