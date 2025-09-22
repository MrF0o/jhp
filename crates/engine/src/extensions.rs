use jhp_executor::BindingInstaller;
use libloading::Library;
use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, OsStr};
use std::fs;
use std::os::raw::{c_char, c_uchar};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

// NOTE: legacy C-ABI removed.

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

// NOTE: legacy C-ABI support removed.

pub fn make_v8_func_from_c_v1<'s>(
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
                    // v1 ABI
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

/// Compute a PascalCase object name from a module key (e.g., "sqlite3" -> "Sqlite3").
fn object_name_for(module_key: &str) -> String {
    let mut chars = module_key.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Candidates for a module's folder name on disk (to find JS bootstraps) and library base.
fn module_name_candidates(name: &str) -> Vec<String> {
    let mut cands = vec![name.to_string()];
    // Also try stripping trailing digits (e.g., sqlite3 -> sqlite)
    let stripped = name.trim_end_matches(|c: char| c.is_ascii_digit());
    if stripped != name && !stripped.is_empty() {
        cands.push(stripped.to_string());
    }
    cands
}

/// Find and load a native module by logical name; returns the module object name and an installer
/// that will, when run in a context, create `global[ObjectName]` and attach native functions and
/// execute any JS bootstrap scripts found under the module folder.
pub fn load_module_installer(
    name: &str,
    ext_dir: &Path,
) -> Result<(String, BindingInstaller), String> {
    let obj_name = object_name_for(name);
    let obj_name_for_return = obj_name.clone();
    let candidates = module_name_candidates(name);

    // Locate a .so library: try libjhp_ext_<cand>.so in ext_dir
    let mut lib_path: Option<PathBuf> = None;
    for cand in &candidates {
        let p = ext_dir.join(format!("libjhp_ext_{}.so", cand));
        if p.exists() {
            lib_path = Some(p);
            break;
        }
    }
    let lib_path = lib_path.ok_or_else(|| {
        format!(
            "No native library found for module '{}' in {}",
            name,
            ext_dir.display()
        )
    })?;

    // Load the library and collect function descriptors
    unsafe {
        let lib = match Library::new(&lib_path) {
            Ok(l) => Box::leak(Box::new(l)),
            Err(e) => return Err(format!("Failed to load {}: {}", lib_path.display(), e)),
        };
        let sym_v1 = lib.get::<ExtRegisterV1Fn>(b"jhp_register_v1");
        let reg = match sym_v1 {
            Ok(s) => s(),
            Err(_) => return Err(format!("Missing jhp_register_v1 in {}", lib_path.display())),
        };
        if reg.abi_version != 1 || reg.funcs.is_null() || reg.len == 0 {
            return Err("Unsupported extension ABI or empty function table".to_string());
        }
        let slice = std::slice::from_raw_parts(reg.funcs, reg.len);
        // Capture function entries for later installer use
        let mut funcs: Vec<(String, ExtCallV1)> = Vec::new();
        for fdesc in slice.iter() {
            if fdesc.name.is_null() {
                continue;
            }
            let Ok(name_c) = CStr::from_ptr(fdesc.name).to_str() else {
                continue;
            };
            funcs.push((name_c.to_string(), fdesc.call));
        }
        let free_fn = reg.free_fn;

        // Collect JS bootstraps under ext_dir/<cand>/*.js sorted
        let mut js_files: Vec<(String, String)> = Vec::new(); // (resource, code)
        'outer: for cand in &candidates {
            let dir = ext_dir.join(cand);
            if dir.exists() {
                if let Ok(read) = fs::read_dir(&dir) {
                    let mut paths: Vec<PathBuf> = read
                        .filter_map(Result::ok)
                        .map(|e| e.path())
                        .filter(|p| p.extension() == Some(OsStr::new("js")))
                        .collect();
                    paths.sort();
                    for p in paths {
                        if let Ok(code) = fs::read_to_string(&p) {
                            js_files.push((p.display().to_string(), code));
                        }
                    }
                }
                // use first found module dir only
                break 'outer;
            }
        }

        // Build installer
        let obj_name_cloned = obj_name.clone();
        let installer: BindingInstaller = Arc::new(move |scope| {
            let global = scope.get_current_context().global(scope);
            let key = v8::String::new(scope, &obj_name_cloned).unwrap();
            let maybe_existing = global.get(scope, key.into());
            let module_obj: v8::Local<v8::Object> = if let Some(val) = maybe_existing {
                val.try_into().unwrap_or_else(|_| v8::Object::new(scope))
            } else {
                v8::Object::new(scope)
            };
            // Attach functions under module object
            for (fname, fptr) in &funcs {
                let f = make_v8_func_from_c_v1(scope, *fptr, free_fn);
                let fkey = v8::String::new(scope, fname).unwrap();
                let _ = module_obj.set(scope, fkey.into(), f.into());
            }
            // Set module object on global in case it wasn't there
            let key = v8::String::new(scope, &obj_name_cloned).unwrap();
            let _ = global.set(scope, key.into(), module_obj.into());

            // Execute JS bootstraps (if any)
            for (resource, code) in &js_files {
                let _ = jhp_executor::v8utils::compile_and_run_current(scope, code, resource);
            }
        });
        Ok((obj_name_for_return, installer))
    }
}

/// A registry of lazily loaded modules shared across executors.
#[derive(Default)]
pub struct ModuleRegistry {
    ext_dir: PathBuf,
    loaded: RwLock<HashSet<String>>, // module keys requested (e.g., "sqlite3")
    installers: RwLock<HashMap<String, BindingInstaller>>, // key -> installer
    obj_names: RwLock<HashMap<String, String>>, // key -> object name (e.g., Sqlite3)
}

impl ModuleRegistry {
    pub fn new<P: Into<PathBuf>>(ext_dir: P) -> Self {
        Self {
            ext_dir: ext_dir.into(),
            ..Default::default()
        }
    }

    /// Ensure a module is loaded; if newly loaded, returns its installer for immediate use.
    pub fn ensure_loaded(&self, key: &str) -> Result<Option<BindingInstaller>, String> {
        {
            let loaded = self.loaded.read().unwrap();
            if loaded.contains(key) {
                return Ok(None);
            }
        }
        // Upgrade to write and double-check
        let mut loaded_w = self.loaded.write().unwrap();
        if loaded_w.contains(key) {
            return Ok(None);
        }
        let (obj_name, installer) = load_module_installer(key, &self.ext_dir)?;
        self.obj_names
            .write()
            .unwrap()
            .insert(key.to_string(), obj_name);
        self.installers
            .write()
            .unwrap()
            .insert(key.to_string(), installer.clone());
        loaded_w.insert(key.to_string());
        Ok(Some(installer))
    }

    pub fn install_all(&self, scope: &mut v8::ContextScope<v8::HandleScope>) {
        let installers = self.installers.read().unwrap();
        for installer in installers.values() {
            installer(scope);
        }
    }

    pub fn install_one(&self, key: &str, scope: &mut v8::ContextScope<v8::HandleScope>) {
        if let Some(installer) = self.installers.read().unwrap().get(key) {
            installer(scope);
        }
    }

    pub fn object_name(&self, key: &str) -> Option<String> {
        self.obj_names.read().unwrap().get(key).cloned()
    }
}
