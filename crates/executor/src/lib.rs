use jhp_parser::CodeBlock;
use std::sync::{Arc, Mutex, Once};
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
    pub buffer: Arc<Mutex<String>>,
    pub receiver: mpsc::Receiver<Op>,
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
        let isolate = v8::Isolate::new(Default::default());

        Self {
            id,
            isolate,
            buffer: Arc::new(Mutex::new(String::new())),
            receiver,
            installers,
        }
    }

    pub async fn run(&mut self) {
        while let Some(op) = self.receiver.recv().await {
            match op {
                Op::Javascript(code) => {
                    let scope = &mut v8::HandleScope::new(&mut self.isolate);
                    let context = v8::Context::new(scope, v8::ContextOptions::default());
                    let mut context_scope = &mut v8::ContextScope::new(scope, context);

                    // Run binding installers first
                    for install in self.installers.iter() {
                        install(&mut context_scope);
                    }

                    if let Err(e) = Self::install_echo_fn(&mut context_scope, self.buffer.clone()) {
                        eprintln!("install_echo_fn error: {}", e);
                        continue;
                    }

                    match Self::compile_script(&mut context_scope, &code) {
                        Ok(script) => match Self::run_script(&mut context_scope, script) {
                            Ok(_) => (),
                            Err(e) => eprintln!("run_script error: {}", e),
                        },
                        Err(e) => eprintln!("compile_script error: {}", e),
                    }
                }
                Op::Render {
                    blocks,
                    resource_name,
                    respond_to,
                } => {
                    let scope = &mut v8::HandleScope::new(&mut self.isolate);
                    let context = v8::Context::new(scope, v8::ContextOptions::default());
                    let mut context_scope = &mut v8::ContextScope::new(scope, context);

                    // install bindings first
                    for install in self.installers.iter() {
                        install(&mut context_scope);
                    }

                    let result = (|| -> Result<String, String> {
                        // start from a clean buffer for each request
                        {
                            let mut guard = self
                                .buffer
                                .lock()
                                .map_err(|_| "buffer poisoned".to_string())?;
                            guard.clear();
                        }
                        Self::install_echo_fn(&mut context_scope, self.buffer.clone())?;
                        // each page is rendered independently in a big js script
                        let js_program = jhp_parser::blocks_to_js(blocks);
                        match crate::v8utils::compile_script(
                            &mut context_scope,
                            &js_program,
                            &resource_name,
                        ) {
                            Ok(script) => {
                                if script.run(&mut context_scope).is_none() {
                                    if let Ok(mut guard) = self.buffer.lock() {
                                        guard.push_str(
                                            "\n<!-- ERROR -->\nScript execution failed\n",
                                        );
                                    }
                                }
                            }
                            Err(e) => {
                                if let Ok(mut guard) = self.buffer.lock() {
                                    guard.push_str(&format!("\n<!-- ERROR -->\n{}\n", e));
                                }
                            }
                        }

                        let guard = self
                            .buffer
                            .lock()
                            .map_err(|_| "buffer poisoned".to_string())?;
                        let out = guard.clone();
                        Ok(out)
                    })();

                    let _ = respond_to.send(result.unwrap_or_else(|e| format!("Error: {}", e)));
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
        output_buffer: Arc<Mutex<String>>,
    ) -> Result<(), String> {
        // Safety: the Arc lives in self, so the Mutex address stays valid while the function exists.
        let mutex_ptr: *const Mutex<String> = Arc::as_ptr(&output_buffer);
        let external_ptr = mutex_ptr as *mut std::ffi::c_void;

        let global = scope.get_current_context().global(scope);

        let echo_fn = v8::Function::builder(
            move |scope: &mut v8::HandleScope,
                  args: v8::FunctionCallbackArguments,
                  _rv: v8::ReturnValue| {
                let data = args.data();
                if let Some(external) = v8::Local::<v8::External>::try_from(data).ok() {
                    // Recover the Mutex<String> pointer and lock it
                    let buf_mutex = unsafe { &*(external.value() as *const Mutex<String>) };
                    if let Some(arg) = args.get(0).to_string(scope) {
                        if let Ok(mut guard) = buf_mutex.lock() {
                            guard.push_str(&arg.to_rust_string_lossy(scope));
                        }
                    }
                } else {
                    eprintln!("Function data is not an External!");
                }
            },
        )
        .data(v8::External::new(scope, external_ptr).into()) // attach External
        .build(scope)
        .ok_or_else(|| "Failed to create echo function".to_string())?;

        let echo_fn_key = v8::String::new(scope, "echo").unwrap();
        global.set(scope, echo_fn_key.into(), echo_fn.into());

        Ok(())
    }
}
