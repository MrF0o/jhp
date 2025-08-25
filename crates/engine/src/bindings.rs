//! Bindings installed into the JS runtime.
//! - `global`: alias to globalThis
//! - `include(path)`: include and execute a file inline. Supports `.jhp` and `.js`.

use jhp_executor::{BindingInstaller, v8utils};
use jhp_parser as parser;
use std::sync::Arc;
use std::{fs, path::Path};

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
pub struct IncludeBinding;

impl InstallBindings for IncludeBinding {
    fn install(&self, scope: &mut v8::ContextScope<v8::HandleScope>) {
        let global = scope.get_current_context().global(scope);

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
                let content = match fs::read_to_string(path_ref).or_else(|_| {
                    let fallback = Path::new("jhp-tests").join(&path);
                    fs::read_to_string(&fallback)
                }) {
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
        .build(scope)
        .expect("Failed to create include function");

        if let Some(key) = v8::String::new(scope, "include") {
            let _ = global.set(scope, key.into(), include_fn.into());
        }
    }
}

/// returns the default set of binding installers used by the engine
pub fn default_installers() -> Vec<BindingInstaller> {
    vec![
        Arc::new(|scope: &mut v8::ContextScope<v8::HandleScope>| {
            GlobalBinding.install(scope);
        }),
        Arc::new(|scope: &mut v8::ContextScope<v8::HandleScope>| {
            IncludeBinding.install(scope);
        }),
    ]
}
