use jhp_parser::CodeBlock;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Once};
use tokio::sync::{mpsc, oneshot};

pub mod v8utils;

pub enum Op {
    Javascript(String),
    Shutdown,
    Render {
        blocks: Vec<Box<CodeBlock>>,
        resource_name: String,
        respond_to: oneshot::Sender<String>,
    },
}

pub struct Executor {
    pub id: usize,
    pub isolate: v8::OwnedIsolate,
    pub receiver: mpsc::Receiver<Op>,
    // Hold no long-lived context; we create a fresh one per request to avoid identifier redeclarations.
    context: v8::Global<v8::Context>,
    installers: Arc<Vec<BindingInstaller>>,
}

/// A binding installer is a function that gets a chance to attach globals/APIs to the context
pub type BindingInstaller =
    Arc<dyn Fn(&mut v8::ContextScope<v8::HandleScope>) + Send + Sync + 'static>;

impl Executor {
    pub fn new(
        id: usize,
        receiver: mpsc::Receiver<Op>,
        installers: Arc<Vec<BindingInstaller>>,
    ) -> Self {
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            let platform = v8::new_default_platform(0, false).make_shared();
            v8::V8::initialize_platform(platform);
            v8::V8::initialize();
        });
        let mut isolate = v8::Isolate::new(Default::default());

        // create a bootstrap context to run installers that shouldn't depend on per-request state
        let installers_for_init = installers.clone();
        let context_global = {
            // create context and set up globals
            let hs1 = &mut v8::HandleScope::new(&mut isolate);
            let context_local = v8::Context::new(hs1, v8::ContextOptions::default());
            {
                let mut cs = v8::ContextScope::new(hs1, context_local);

                // install all bindings once per executor
                for install in installers_for_init.iter() {
                    install(&mut cs);
                }
            }
            // create a Global from the same handlescope after cs dropped
            v8::Global::new(hs1, context_local)
        };

        Self {
            id,
            isolate,
            receiver,
            context: context_global,
            installers,
        }
    }

    pub async fn run(&mut self) {
        while let Some(op) = self.receiver.recv().await {
            match op {
                Op::Javascript(code) => {
                    let hs = &mut v8::HandleScope::new(&mut self.isolate);
                    let context = v8::Local::new(hs, &self.context);
                    let mut cs = v8::ContextScope::new(hs, context);

                    match Self::compile_script(&mut cs, &code) {
                        Ok(script) => {
                            if let Err(e) = Self::run_script(&mut cs, script) {
                                eprintln!("run_script error: {}", e)
                            }
                        }
                        Err(e) => eprintln!("compile_script error: {}", e),
                    }
                }
                Op::Render {
                    blocks,
                    resource_name,
                    respond_to,
                } => {
                    // create a fresh context per render to avoid re-declaration conflicts
                    let hs = &mut v8::HandleScope::new(&mut self.isolate);

                    // derive a new context so that each request has isolated globals
                    let mut req_scope = {
                        let context_local = v8::Context::new(hs, v8::ContextOptions::default());
                        v8::ContextScope::new(hs, context_local)
                    };

                    // reinstall bindings that should exist in each fresh request context
                    for install in self.installers.iter() {
                        install(&mut req_scope);
                    }

                    // install per-request echo bound to a fresh buffer
                    let buffer: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
                    if let Err(e) = Self::install_echo_fn(&mut req_scope, buffer.clone()) {
                        eprintln!("install_echo_fn error: {}", e);
                    }

                    // execute each JHP block; HTML bypasses V8 for speed
                    let _ = crate::v8utils::run_jhp_blocks_with_origin(
                        &mut req_scope,
                        blocks,
                        &resource_name,
                        buffer.clone(),
                    );

                    let out = buffer.borrow().clone();
                    let _ = respond_to.send(out);
                }
                Op::Shutdown => break,
            }
        }
    }

    fn compile_script<'s>(
        scope: &mut v8::ContextScope<v8::HandleScope<'s>>,
        code: &str,
    ) -> Result<v8::Local<'s, v8::Script>, String> {
        crate::v8utils::compile_script(scope, code, "index.jhp")
    }

    fn run_script<'s>(
        scope: &mut v8::ContextScope<v8::HandleScope<'s>>,
        script: v8::Local<'s, v8::Script>,
    ) -> Result<v8::Local<'s, v8::Value>, String> {
        script
            .run(scope)
            .ok_or_else(|| "Failed to run the js code block sources".to_string())
    }

    fn install_echo_fn(
        scope: &mut v8::ContextScope<v8::HandleScope>,
        output_buffer: Rc<RefCell<String>>,
    ) -> Result<(), String> {
        // SAFETY: the Rc lives until end of request; we only use it within this context's lifetime.
        let ptr: *const RefCell<String> = Rc::as_ptr(&output_buffer);
        let external_ptr = ptr as *mut std::ffi::c_void;

        let global = scope.get_current_context().global(scope);

        let echo_fn = v8::Function::builder(
            move |scope: &mut v8::HandleScope,
                  args: v8::FunctionCallbackArguments,
                  _rv: v8::ReturnValue| {
                let data = args.data();
                if let Some(external) = v8::Local::<v8::External>::try_from(data).ok() {
                    // Recover the Mutex<String> pointer and lock it
                    let buf_cell = unsafe { &*(external.value() as *const RefCell<String>) };
                    if let Some(arg) = args.get(0).to_string(scope) {
                        buf_cell
                            .borrow_mut()
                            .push_str(&arg.to_rust_string_lossy(scope));
                    }
                } else {
                    eprintln!("Function data is not an External!");
                }
            },
        )
        .data(v8::External::new(scope, external_ptr).into())
        .build(scope)
        .ok_or_else(|| "Failed to create echo function".to_string())?;

        let echo_fn_key = v8::String::new(scope, "echo").unwrap();
        global.set(scope, echo_fn_key.into(), echo_fn.into());

        Ok(())
    }
}
