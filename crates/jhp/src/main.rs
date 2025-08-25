use jhp_engine::engine::Engine;

#[tokio::main]
async fn main() {
    let mut engine = Engine::new(1);
    engine.run().await.unwrap();
}
