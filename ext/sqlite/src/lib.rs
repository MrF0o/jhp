use jhp_extensions::{JhpBuf, JhpCallResult, ok_json};

extern "C" fn sqlite_test(_buf: JhpBuf) -> JhpCallResult {
    ok_json(&serde_json::json!({"message": "It works!"}))
}

jhp_extensions::export_jhp_v1! {
    "sqlite_test" => sqlite_test
}
