use crate::config::HttpServerConfig;
use axum::{Router, response::Html, routing::get};
use jhp_executor::Op;
use jhp_parser as parser;
use std::fs;
use std::sync::Arc;
use tokio::sync::mpsc;

#[derive(Clone)]
pub struct HttpServer {
    router: Arc<Router>,
    config: HttpServerConfig,
}

impl HttpServer {
    /// Construct an HttpServer with routes defined here.
    /// By default exposes:
    /// - GET "/": renders `jhp-tests/index.jhp` via the executor.
    pub fn new(sender: mpsc::Sender<Op>, config: HttpServerConfig) -> Self {
        let index_path = config.index_path();
        let index_name = config.index_file.clone();
        let router = Router::new().route(
            "/",
            get({
                let sender = sender.clone();
                let index_path = index_path.clone();
                let index_name = index_name.clone();
                move || {
                    let sender = sender.clone();
                    let index_path = index_path.clone();
                    let index_name = index_name.clone();
                    async move {
                        let content = match fs::read_to_string(&index_path) {
                            Ok(content) => content,
                            Err(_) => return Html("Template file not found".to_string()),
                        };

                        let mut p = parser::Parser::new(&content);
                        let res = p.parse();
                        let blocks = res.blocks;

                        let (tx, rx) = tokio::sync::oneshot::channel();
                        let _ = sender
                            .send(Op::Render {
                                blocks,
                                resource_name: index_name.clone(),
                                respond_to: tx,
                            })
                            .await;

                        match rx.await {
                            Ok(body) => Html(body),
                            Err(_) => Html("Executor unavailable".to_string()),
                        }
                    }
                }
            }),
        );

        Self {
            router: Arc::new(router),
            config,
        }
    }

    pub async fn start(&self) {
        let listener = tokio::net::TcpListener::bind(&self.config.addr())
            .await
            .unwrap();

        axum::serve(listener, (*self.router).clone()).await.unwrap();
    }
}
