use axum::Router;
use std::sync::Arc;

#[derive(Clone)]
pub struct HttpServer {
    router: Arc<Router>,
}

impl HttpServer {
    pub fn new(router: Router) -> Self {
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
