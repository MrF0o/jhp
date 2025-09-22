//! Bindings installed into the JS runtime.
//! - `global`: alias to globalThis
//! - `include(path)`: include and execute a file inline. Supports `.jhp` and `.js`.
//!   If `path` has no extension, it is treated as a module name and we attempt to
//!   resolve `<name>.js` from the document root or the extensions directory.

use crate::config::EngineConfig;
use crate::extensions::ModuleRegistry;
use jhp_executor::BindingInstaller;
use jhp_parser as parser;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub trait InstallBindings {
    fn install(&self, scope: &mut v8::ContextScope<v8::HandleScope>);
}

/// Installs a `global` alias pointing to the context's global object.
pub struct GlobalBinding;

impl InstallBindings for GlobalBinding {
    fn install(&self, scope: &mut v8::ContextScope<v8::HandleScope>) {
        let global = scope.get_current_context().global(scope);
        if let Some(key) = v8::String::new(scope, "global") {
            let _ = global.set(scope, key.into(), global.into());
        }
    }
}

/// Installs an `include(path)` function to inline-execute files.
/// - If `path` ends with `.jhp`, the file is parsed with the JHP parser and transformed to JS.
/// - If `path` ends with `.js`, the file contents are executed directly.
pub struct IncludeBinding {
    /// Fallback directory to resolve includes relative to when a provided path
    /// isn't directly readable.
    pub document_root: PathBuf,
    /// Directory containing extensions (native `.so` and JS shims). When `include()`
    /// is called with a bare module name (no extension), we'll look for `<name>.js`
    /// here as a convenience, e.g. `include('sqlite3')` resolves to `ext/sqlite3.js`
    /// or `ext/sqlite/sqlite3.js`.
    pub extensions_dir: PathBuf,
    /// Shared registry for lazy-loading native modules.
    pub modules: Arc<ModuleRegistry>,
}

impl IncludeBinding {
    pub fn new<P: Into<PathBuf>, Q: Into<PathBuf>>(
        document_root: P,
        extensions_dir: Q,
        modules: Arc<ModuleRegistry>,
    ) -> Self {
        Self {
            document_root: document_root.into(),
            extensions_dir: extensions_dir.into(),
            modules,
        }
    }
}

impl InstallBindings for IncludeBinding {
    fn install(&self, scope: &mut v8::ContextScope<v8::HandleScope>) {
        let global = scope.get_current_context().global(scope);

        // Pass state via External pointer into the callback to satisfy V8's callback requirements
        #[repr(C)]
        struct IncludeState {
            doc_root: PathBuf,
            ext_dir: PathBuf,
            modules: Arc<ModuleRegistry>,
        }
        let state = IncludeState {
            doc_root: self.document_root.clone(),
            ext_dir: self.extensions_dir.clone(),
            modules: self.modules.clone(),
        };
        let state_ptr = Box::into_raw(Box::new(state)) as *mut std::ffi::c_void;
        let external = v8::External::new(scope, state_ptr);

        let include_fn = v8::Function::builder(
            move |scope: &mut v8::HandleScope,
                  args: v8::FunctionCallbackArguments,
                  mut rv: v8::ReturnValue| {
                // path argument
                let path_val = args.get(0);
                let Some(path_str) = path_val.to_string(scope) else {
                    let msg =
                        v8::String::new(scope, "include(path): path must be a string").unwrap();
                    let exc = v8::Exception::type_error(scope, msg);
                    scope.throw_exception(exc);
                    return;
                };
                let path = path_str.to_rust_string_lossy(scope);

                // If no extension, treat as a potential native module first.
                let has_ext = Path::new(&path).extension().is_some();
                if !has_ext {
                    // Try to lazy-load module by name
                    let st_ptr = v8::Local::<v8::External>::try_from(args.data())
                        .map(|e| e.value() as *const IncludeState)
                        .unwrap();
                    let st: &IncludeState = unsafe { &*st_ptr };
                    match st.modules.ensure_loaded(&path) {
                        Ok(Some(_)) => {
                            // Newly loaded: install just this module into current context
                            let context = scope.get_current_context();
                            let mut cs = v8::ContextScope::new(scope, context);
                            st.modules.install_one(&path, &mut cs);
                        }
                        Ok(None) => {
                            // Already loaded: ensure installed in this context
                            let context = scope.get_current_context();
                            let mut cs = v8::ContextScope::new(scope, context);
                            st.modules.install_one(&path, &mut cs);
                        }
                        Err(_e) => {
                            // Not a native module; fall through to file resolution below
                        }
                    }
                    // If module object now exists, return it.
                    if let Some(obj_name) = st.modules.object_name(&path) {
                        if let Some(key) = v8::String::new(scope, &obj_name) {
                            let g = scope.get_current_context().global(scope);
                            if let Some(val) = g.get(scope, key.into()) {
                                rv.set(val);
                                return;
                            }
                        }
                    }
                    // else: proceed to try JS shim resolution
                }

                // resolve and load file content as before
                let path_ref = Path::new(&path);
                let mut content: Option<String> = None;
                if content.is_none() {
                    if let Ok(s) = fs::read_to_string(path_ref) {
                        content = Some(s);
                    }
                }
                if content.is_none() {
                    let st_ptr = v8::Local::<v8::External>::try_from(args.data())
                        .map(|e| e.value() as *const IncludeState)
                        .unwrap();
                    let st: &IncludeState = unsafe { &*st_ptr };
                    if let Ok(s) = fs::read_to_string(st.doc_root.join(&path)) {
                        content = Some(s);
                    }
                }
                if !has_ext {
                    let name = &path;
                    let st_ptr = v8::Local::<v8::External>::try_from(args.data())
                        .map(|e| e.value() as *const IncludeState)
                        .unwrap();
                    let st: &IncludeState = unsafe { &*st_ptr };
                    let candidates = [
                        st.doc_root.join(format!("{}.js", name)),
                        st.ext_dir.join(name).join(format!("{}.js", name)),
                        st.ext_dir.join(format!("{}.js", name)),
                    ];
                    for p in candidates.iter() {
                        if content.is_none() {
                            if let Ok(s) = fs::read_to_string(p) {
                                content = Some(s);
                                break;
                            }
                        }
                    }
                }
                let Some(content) = content else {
                    let msg = v8::String::new(
                        scope,
                        &format!(
                            "include('{}') read error: not found as module or file",
                            path
                        ),
                    )
                    .unwrap();
                    let exc = v8::Exception::error(scope, msg);
                    scope.throw_exception(exc);
                    return;
                };

                // execute..
                let result_val: Option<v8::Local<v8::Value>> = if path.ends_with(".jhp") {
                    let mut p = parser::Parser::new(&content);
                    let res = p.parse();
                    let js = parser::blocks_to_js(res.blocks);
                    // compile+run and capture result
                    let context = scope.get_current_context();
                    let mut cs = v8::ContextScope::new(scope, context);
                    let src = v8::String::new(&mut cs, &js).unwrap();
                    let name = v8::String::new(&mut cs, &path).unwrap();
                    let origin = v8::ScriptOrigin::new(
                        &mut cs,
                        name.into(),
                        0,
                        0,
                        false,
                        0,
                        None,
                        false,
                        false,
                        false,
                        None,
                    );
                    match v8::Script::compile(&mut cs, src, Some(&origin))
                        .and_then(|s| s.run(&mut cs))
                    {
                        Some(v) => Some(v),
                        None => None,
                    }
                } else if path.ends_with(".js") {
                    let context = scope.get_current_context();
                    let mut cs = v8::ContextScope::new(scope, context);
                    let src = v8::String::new(&mut cs, &content).unwrap();
                    let name = v8::String::new(&mut cs, &path).unwrap();
                    let origin = v8::ScriptOrigin::new(
                        &mut cs,
                        name.into(),
                        0,
                        0,
                        false,
                        0,
                        None,
                        false,
                        false,
                        false,
                        None,
                    );
                    match v8::Script::compile(&mut cs, src, Some(&origin))
                        .and_then(|s| s.run(&mut cs))
                    {
                        Some(v) => Some(v),
                        None => None,
                    }
                } else {
                    // Treated as module shim (no extension), run as JS and return value
                    let context = scope.get_current_context();
                    let mut cs = v8::ContextScope::new(scope, context);
                    let src = v8::String::new(&mut cs, &content).unwrap();
                    let name = v8::String::new(&mut cs, &format!("{}.js", path)).unwrap();
                    let origin = v8::ScriptOrigin::new(
                        &mut cs,
                        name.into(),
                        0,
                        0,
                        false,
                        0,
                        None,
                        false,
                        false,
                        false,
                        None,
                    );
                    match v8::Script::compile(&mut cs, src, Some(&origin))
                        .and_then(|s| s.run(&mut cs))
                    {
                        Some(v) => Some(v),
                        None => None,
                    }
                };

                if let Some(v) = result_val {
                    rv.set(v);
                } else {
                    // error already thrown by V8; ensure we return undefined
                    rv.set(v8::undefined(scope).into());
                }
            },
        )
        .data(external.into())
        .build(scope)
        .expect("Failed to create include function");

        if let Some(key) = v8::String::new(scope, "include") {
            let _ = global.set(scope, key.into(), include_fn.into());
        }
    }
}

/// Build the default set of binding installers used by the engine, configured with a document root.
pub fn default_installers(
    cfg: &EngineConfig,
    modules: Arc<ModuleRegistry>,
) -> Vec<BindingInstaller> {
    let document_root = cfg.document_root.clone();
    let extensions_dir = cfg.extensions_dir.clone();
    vec![
        Arc::new(|scope: &mut v8::ContextScope<v8::HandleScope>| {
            GlobalBinding.install(scope);
        }),
        {
            // Ensure any modules that have been lazily loaded are installed for each context
            let modules = modules.clone();
            Arc::new(move |scope: &mut v8::ContextScope<v8::HandleScope>| {
                modules.install_all(scope);
            })
        },
        {
            let tr_doc = document_root.clone();
            let tr_ext = extensions_dir.clone();
            let modules = modules.clone();
            Arc::new(move |scope: &mut v8::ContextScope<v8::HandleScope>| {
                IncludeBinding::new(tr_doc.clone(), tr_ext.clone(), modules.clone()).install(scope);
            })
        },
    ]
}
