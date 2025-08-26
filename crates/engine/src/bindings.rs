//! Bindings installed into the JS runtime.
//! - `global`: alias to globalThis
//! - `include(path)`: include and execute a file inline. Supports `.jhp` and `.js`.

use jhp_executor::{BindingInstaller, v8utils};
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
}

impl IncludeBinding {
    pub fn new<P: Into<PathBuf>>(document_root: P) -> Self {
        Self {
            document_root: document_root.into(),
        }
    }
}

impl InstallBindings for IncludeBinding {
    fn install(&self, scope: &mut v8::ContextScope<v8::HandleScope>) {
        let global = scope.get_current_context().global(scope);

        // Pass document_root via External to avoid capturing non-Copy in the callback
        let tr_ptr: *mut std::ffi::c_void =
            Box::into_raw(Box::new(self.document_root.clone())) as *mut std::ffi::c_void;
        let external = v8::External::new(scope, tr_ptr);

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

                // load file
                let path_ref = Path::new(&path);
                // retrieve document_root from function data
                let tr_buf = v8::Local::<v8::External>::try_from(args.data())
                    .map(|e| e.value() as *const PathBuf)
                    .unwrap();
                let document_root: &PathBuf = unsafe { &*tr_buf };

                let content = match fs::read_to_string(path_ref)
                    .or_else(|_| fs::read_to_string(document_root.join(&path)))
                {
                    Ok(s) => s,
                    Err(e) => {
                        let msg = v8::String::new(
                            scope,
                            &format!("include('{}') read error: {}", path, e),
                        )
                        .unwrap();
                        let exc = v8::Exception::error(scope, msg);
                        scope.throw_exception(exc);
                        return;
                    }
                };

                // execute..
                let result = if path.ends_with(".jhp") {
                    let mut p = parser::Parser::new(&content);
                    let res = p.parse();
                    let js = parser::blocks_to_js(res.blocks);
                    v8utils::compile_and_run_current(scope, &js, &path)
                } else if path.ends_with(".js") {
                    v8utils::compile_and_run_current(scope, &content, &path)
                } else {
                    Err("include(path): only .jhp or .js supported".to_string())
                };

                if let Err(e) = result {
                    let msg = v8::String::new(scope, &format!("include('{}') failed: {}", path, e))
                        .unwrap();
                    let exc = v8::Exception::error(scope, msg);
                    scope.throw_exception(exc);
                    return;
                }

                rv.set(v8::undefined(scope).into());
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
pub fn default_installers<P: AsRef<Path>>(document_root: P) -> Vec<BindingInstaller> {
    let document_root = document_root.as_ref().to_path_buf();
    vec![
        Arc::new(|scope: &mut v8::ContextScope<v8::HandleScope>| {
            GlobalBinding.install(scope);
        }),
        {
            let tr = document_root.clone();
            Arc::new(move |scope: &mut v8::ContextScope<v8::HandleScope>| {
                IncludeBinding::new(tr.clone()).install(scope);
            })
        },
    ]
}
