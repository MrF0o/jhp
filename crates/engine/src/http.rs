use axum::{Router, response::Html, routing::get};
use jhp_executor::Op;
use jhp_parser as parser;
use std::fs;
use std::sync::Arc;
use tokio::sync::mpsc;

#[derive(Clone)]
pub struct HttpServer {
    router: Arc<Router>,
}

impl HttpServer {
    /// Construct an HttpServer with routes defined here.
    /// Currently exposes:
    /// - GET "/": renders `jhp-tests/index.jhp` via the executor.
    pub fn new(sender: mpsc::Sender<Op>) -> Self {
        let router = Router::new().route(
            "/",
            get({
                let sender = sender.clone();
                move || {
                    let sender = sender.clone();
                    async move {
                        let filepath = String::from("jhp-tests/index.jhp");
                        let content = match fs::read_to_string(&filepath) {
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
                                resource_name: "index.jhp".to_string(),
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
        }
    }

    pub async fn start(&self) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:3000")
            .await
            .unwrap();

        axum::serve(listener, (*self.router).clone()).await.unwrap();
    }
}
