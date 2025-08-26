pub mod sqlite;

use serde_json::Value;

#[derive(Clone, Debug)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>, // row-major
}

pub trait SqliteDriver: Send + Sync + 'static {
    type Pool: Clone + Send + Sync + 'static;
    fn connect(url: &str) -> Result<Self::Pool, sqlx::Error>;
    fn query(pool: &Self::Pool, sql: &str, params: &[Value]) -> Result<QueryResult, sqlx::Error>;
}
