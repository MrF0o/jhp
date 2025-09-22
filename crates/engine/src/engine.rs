use crate::config::EngineConfig;
use crate::http::HttpServer;
use crate::{bindings, extensions};
use jhp_executor::{BindingInstaller, Executor, Op};
use std::sync::Arc;
use std::thread::JoinHandle;
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
    pub modules: Arc<extensions::ModuleRegistry>,
}

impl ExecutorPool {
    pub fn new(nb: usize, config: &EngineConfig) -> Self {
        let mut threads = Vec::with_capacity(nb);
        let mut senders = Vec::with_capacity(nb);

        // Shared module registry for lazy loading
        let modules: Arc<extensions::ModuleRegistry> =
            Arc::new(extensions::ModuleRegistry::new(&config.extensions_dir));

        // Prepare installers: built-ins + include (uses modules). Do NOT eagerly load .so or .js.
        let all_installers: Vec<BindingInstaller> =
            bindings::default_installers(&config, modules.clone());
        let installers: Arc<Vec<BindingInstaller>> = Arc::new(all_installers);

        for id in 0..nb {
            // each executor gets its own channel
            // Use a deeper mailbox to absorb bursts from the HTTP server under load.
            // This reduces contention and awaits in the forwarder at high concurrency.
            let (tx, rx) = mpsc::channel::<Op>(1024);
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
            modules,
        }
    }

    pub async fn send(&self, op: Op) -> Result<(), mpsc::error::SendError<Op>> {
        let len = self.senders.len();
        let idx = self.next_idx.fetch_add(1, Ordering::Relaxed) % len.max(1);
        self.senders[idx].send(op).await
    }

    /// Consume ops from a central channel and dispatch to executors in round-robin
    pub async fn forward(&self, mut rx: mpsc::UnboundedReceiver<Op>) {
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
    sender: mpsc::UnboundedSender<Op>,
    receiver: Option<mpsc::UnboundedReceiver<Op>>,
    pub config: EngineConfig,
}

impl Engine {
    pub fn new(nb_executors: usize) -> Self {
        Self::new_with_config(nb_executors, EngineConfig::default())
    }

    pub fn new_with_config(nb_executors: usize, config: EngineConfig) -> Self {
        assert!(nb_executors > 0);
        let pool = std::sync::Arc::new(ExecutorPool::new(nb_executors, &config));
        let (sender, receiver) = mpsc::unbounded_channel::<Op>();

        Self {
            executor_pool: pool,
            sender,
            receiver: Some(receiver),
            config,
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

        let task = tokio::spawn({
            let server = HttpServer::new(self.sender.clone(), self.config.http());
            async move { server.start().await }
        });
        task.await.unwrap();
        Ok(())
    }
}
