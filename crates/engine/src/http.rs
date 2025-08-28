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
        // Root path: empty or only slashes -> render index or 404
        if path.trim_matches('/').is_empty() {
            match doc_root.read_index().await {
                Ok(content) => {
                    let mut p = parser::Parser::new(&content);
                    let blocks = p.parse().blocks;
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    let _ = sender.send(Op::Render {
                        blocks,
                        resource_name: doc_root.index_name().to_string(),
                        respond_to: tx,
                    });
                    return match rx.await {
                        Ok(body) => Html(body).into_response(),
                        Err(_) => (StatusCode::SERVICE_UNAVAILABLE, "Executor unavailable")
                            .into_response(),
                    };
                }
                Err(_) => {
                    return (StatusCode::NOT_FOUND, "Cannot get '/': File Not Found")
                        .into_response();
                }
            }
        }

        let rel = path.trim_start_matches('/');
        if rel.contains("..") {
            return (StatusCode::FORBIDDEN, "Invalid path").into_response();
        }

        if rel == doc_root.index_name() {
            match doc_root.read_index().await {
                Ok(content) => {
                    let mut p = parser::Parser::new(&content);
                    let blocks = p.parse().blocks;
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    let _ = sender.send(Op::Render {
                        blocks,
                        resource_name: doc_root.index_name().to_string(),
                        respond_to: tx,
                    });
                    return match rx.await {
                        Ok(body) => Html(body).into_response(),
                        Err(_) => (StatusCode::SERVICE_UNAVAILABLE, "Executor unavailable")
                            .into_response(),
                    };
                }
                Err(_) => {
                    return (StatusCode::NOT_FOUND, "Cannot get '/': File Not Found")
                        .into_response();
                }
            }
        }

        // Read once and decide path based on suffix
        match doc_root.read_file(rel).await {
            Ok(content) => {
                if rel.ends_with(".jhp") {
                    let mut p = parser::Parser::new(&content);
                    let blocks = p.parse().blocks;
                    let resource_name = rel.to_string();
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    let _ = sender.send(Op::Render {
                        blocks,
                        resource_name,
                        respond_to: tx,
                    });
                    match rx.await {
                        Ok(body) => Html(body).into_response(),
                        Err(_) => (StatusCode::SERVICE_UNAVAILABLE, "Executor unavailable")
                            .into_response(),
                    }
                } else {
                    Html(content).into_response()
                }
            }
            Err(_) => {
                let msg = format!("Cannot get '/{}': File Not Found", rel);
                (StatusCode::NOT_FOUND, msg).into_response()
            }
        }
    }

    pub async fn start(&self) {
        let listener = tokio::net::TcpListener::bind(&self.config.addr())
            .await
            .unwrap();

        axum::serve(listener, (*self.router).clone()).await.unwrap();
    }
}
