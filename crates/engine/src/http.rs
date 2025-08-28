use crate::config::HttpServerConfig;
use crate::fs::DocumentRoot;
use axum::{
    Router,
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::get,
};
use jhp_executor::Op;
use jhp_parser as parser;
use std::sync::Arc;
use tokio::sync::mpsc;

#[derive(Clone)]
pub struct HttpServer {
    router: Arc<Router>,
    config: HttpServerConfig,
}

pub struct HttpRequest;
pub struct HttpRespnse;

impl HttpServer {
    /// Construct an HttpServer with routes defined here.
    /// By default exposes:
    /// - GET "/": renders `jhp-tests/index.jhp` via the executor.
    pub fn new(sender: mpsc::UnboundedSender<Op>, config: HttpServerConfig) -> Self {
        let doc_root = DocumentRoot::new(config.document_root.clone(), config.index_file.clone());
        let router = Router::new()
            .route(
                "/",
                get({
                    let sender = sender.clone();
                    let doc_root = doc_root.clone();
                    move || {
                        let sender = sender.clone();
                        let doc_root = doc_root.clone();
                        async move { Self::handle_request(sender, doc_root, String::new()).await }
                    }
                }),
            )
            .route(
                "/{*path}",
                get({
                    let sender = sender.clone();
                    let doc_root = doc_root.clone();
                    move |axum::extract::Path(path): axum::extract::Path<String>| {
                        let sender = sender.clone();
                        let doc_root = doc_root.clone();
                        async move { Self::handle_request(sender, doc_root, path).await }
                    }
                }),
            );

        Self {
            router: Arc::new(router),
            config,
        }
    }

    async fn handle_request(
        sender: mpsc::UnboundedSender<Op>,
        doc_root: DocumentRoot,
        path: String,
    ) -> Response {
        // Root path: empty or only slashes -> render index if present, else 404
        if path.trim_matches('/').is_empty() {
            if !doc_root.root_file_exists(doc_root.index_name()).await {
                return (StatusCode::NOT_FOUND, "Cannot get '/': File Not Found").into_response();
            }

            let content = match doc_root.read_index().await {
                Ok(content) => content,
                Err(_) => {
                    return (StatusCode::NOT_FOUND, "Cannot get '/': File Not Found")
                        .into_response();
                }
            };

            let mut p = parser::Parser::new(&content);
            let res = p.parse();
            let blocks = res.blocks;

            let (tx, rx) = tokio::sync::oneshot::channel();
            let _ = sender.send(Op::Render {
                blocks,
                resource_name: doc_root.index_name().to_string(),
                respond_to: tx,
            });

            return match rx.await {
                Ok(body) => Html(body).into_response(),
                Err(_) => (StatusCode::SERVICE_UNAVAILABLE, "Executor unavailable").into_response(),
            };
        }

        let rel = path.trim_start_matches('/');
        if rel.contains("..") {
            return (StatusCode::FORBIDDEN, "Invalid path").into_response();
        }

        if rel == doc_root.index_name() {
            let content = match doc_root.read_index().await {
                Ok(content) => content,
                Err(_) => {
                    return (StatusCode::NOT_FOUND, "Cannot get '/': File Not Found")
                        .into_response();
                }
            };

            let mut p = parser::Parser::new(&content);
            let res = p.parse();
            let blocks = res.blocks;

            let (tx, rx) = tokio::sync::oneshot::channel();
            let _ = sender.send(Op::Render {
                blocks,
                resource_name: doc_root.index_name().to_string(),
                respond_to: tx,
            });

            return match rx.await {
                Ok(body) => Html(body).into_response(),
                Err(_) => (StatusCode::SERVICE_UNAVAILABLE, "Executor unavailable").into_response(),
            };
        }

        if !doc_root.root_file_exists(rel).await {
            let msg = format!("Cannot get '/{}': File Not Found", rel);
            return (StatusCode::NOT_FOUND, msg).into_response();
        }

        // If a JHP template is requested, render it via the executor
        if rel.ends_with(".jhp") {
            let content = match doc_root.read_file(rel).await {
                Ok(c) => c,
                Err(_) => {
                    return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to read file")
                        .into_response();
                }
            };

            let mut p = parser::Parser::new(&content);
            let res = p.parse();
            let blocks = res.blocks;

            let resource_name = rel.to_string();
            let (tx, rx) = tokio::sync::oneshot::channel();
            let _ = sender.send(Op::Render {
                blocks,
                resource_name,
                respond_to: tx,
            });

            return match rx.await {
                Ok(body) => Html(body).into_response(),
                Err(_) => (StatusCode::SERVICE_UNAVAILABLE, "Executor unavailable").into_response(),
            };
        }

        // Otherwise, serve as a static file
        match doc_root.read_file(rel).await {
            Ok(content) => Html(content).into_response(),
            Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Failed to read file").into_response(),
        }
    }

    pub async fn start(&self) {
        let listener = tokio::net::TcpListener::bind(&self.config.addr())
            .await
            .unwrap();

        axum::serve(listener, (*self.router).clone()).await.unwrap();
    }
}
