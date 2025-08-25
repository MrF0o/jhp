use crate::http::HttpServer;
use crate::{bindings, extensions};
use axum::{Router, response::Html, routing::get};
use jhp_executor::{BindingInstaller, Executor, Op};
use jhp_parser as parser;
use std::sync::Arc;
use std::{fs, thread::JoinHandle};
use std::{
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
};
use tokio::sync::mpsc;

pub struct ExecutorPool {
    senders: Vec<mpsc::Sender<Op>>,
    threads: Mutex<Vec<JoinHandle<()>>>,
    next_idx: AtomicUsize,
}

impl ExecutorPool {
    pub fn new(nb: usize) -> Self {
        let mut threads = Vec::with_capacity(nb);
        let mut senders = Vec::with_capacity(nb);

        // Prepare installers: built-ins + native extensions (.so) + JS extensions (.js)
        let mut all_installers: Vec<BindingInstaller> = bindings::default_installers();
        let ext_dir = std::path::Path::new("ext");
        let native_installers = extensions::load_installers(ext_dir);
        let js_installers = extensions::load_js_installers(ext_dir);
        all_installers.extend(native_installers);
        all_installers.extend(js_installers);
        let installers: Arc<Vec<BindingInstaller>> = Arc::new(all_installers);

        for id in 0..nb {
            // each executor gets its own channel
            let (tx, rx) = mpsc::channel::<Op>(nb);
            senders.push(tx);

            let installers_cloned = installers.clone();
            let handle = thread::spawn(move || {
                let mut executor = Executor::new(id, rx, installers_cloned);
                // create a single-threaded tokio runtime for this thread
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("Failed to create runtime");

                rt.block_on(async move {
                    executor.run().await;
                });
            });
            threads.push(handle);
        }

        ExecutorPool {
            senders,
            threads: Mutex::new(threads),
            next_idx: AtomicUsize::new(0),
        }
    }

    pub async fn send(&self, op: Op) -> Result<(), mpsc::error::SendError<Op>> {
        let len = self.senders.len();
        let idx = self.next_idx.fetch_add(1, Ordering::Relaxed) % len.max(1);
        self.senders[idx].send(op).await
    }

    /// Consume ops from a central channel and dispatch to executors in round-robin
    pub async fn forward(&self, mut rx: mpsc::Receiver<Op>) {
        while let Some(op) = rx.recv().await {
            let _ = self.send(op).await;
        }
    }

    pub fn size(&self) -> usize {
        self.senders.len()
    }

    /// Take ownership of handles, then join outside the lock
    pub fn join(&self) {
        let handles: Vec<JoinHandle<()>> = {
            let mut guard = self.threads.lock().unwrap();
            std::mem::take(&mut *guard)
        };
        for h in handles {
            let _ = h.join();
        }
    }
}

pub struct Engine {
    pub executor_pool: std::sync::Arc<ExecutorPool>,
    sender: mpsc::Sender<Op>,
    receiver: Option<mpsc::Receiver<Op>>,
}

impl Engine {
    pub fn new(nb_executors: usize) -> Self {
        assert!(nb_executors > 0);
        let pool = std::sync::Arc::new(ExecutorPool::new(nb_executors));
        let (sender, receiver) = mpsc::channel::<Op>(128);

        Self {
            executor_pool: pool,
            sender,
            receiver: Some(receiver),
        }
    }

    pub async fn run(&mut self) -> Result<(), String> {
        // spawn two tokio tasks: pool forwarder and HTTP server
        if let Some(rx) = self.receiver.take() {
            let pool = std::sync::Arc::clone(&self.executor_pool);
            tokio::spawn(async move {
                pool.forward(rx).await;
            });
        }

        let sender = self.sender.clone();
        let router = Router::new().route(
            "/",
            get(move || {
                let sender = sender.clone();
                async move {
                    // Read and parse template fresh on each request
                    let filepath = String::from("jhp-tests/index.jhp");
                    let content = match fs::read_to_string(filepath) {
                        Ok(content) => content,
                        Err(_) => return Html("Template file not found".to_string()),
                    };

                    let mut parser = parser::Parser::new(&content);
                    let parser_results = parser.parse();
                    let blocks = parser_results.blocks;

                    let (tx, rx) = tokio::sync::oneshot::channel();
                    let _ = sender
                        .send(Op::Render {
                            blocks,
                            resource_name: "index.jhp".to_string(),
                            respond_to: tx,
                        })
                        .await;
                    match rx.await {
                        Ok(body) => Html(body),
                        Err(_) => Html("Executor unavailable".to_string()),
                    }
                }
            }),
        );

        let task = tokio::spawn({
            let server = HttpServer::new(router);
            async move { server.start().await }
        });
        task.await.unwrap();
        Ok(())
    }
}
