//! `hello-tool`: echoes its input and remembers how often it was called
//! (plugin KV store). Same contract as the WAT fixture in
//! `orkestr_plugins::wat::hello`.

use orkestr_plugin_sdk::CallRequest;
use serde_json::{json, Value};

fn handle(req: CallRequest) -> Result<Value, String> {
    #[cfg(target_arch = "wasm32")]
    {
        use orkestr_plugin_sdk::host;
        host::log("info", "hello called")?;
        let n = host::kv_get("calls")?.as_u64().unwrap_or(0) + 1;
        host::kv_set("calls", &json!(n))?;
    }
    match req.tool.as_str() {
        "hello" => {
            let name = req.input.get("name").and_then(Value::as_str).unwrap_or("world");
            Ok(json!({ "echo": { "tool": req.tool, "input": req.input }, "greeting": format!("Hello, {name}!") }))
        }
        other => Err(format!("unknown tool `{other}`")),
    }
}

#[cfg(target_arch = "wasm32")]
orkestr_plugin_sdk::orkestr_plugin!(handle);

#[cfg(not(target_arch = "wasm32"))]
#[allow(dead_code)]
fn main_like() -> Result<Value, String> {
    handle(CallRequest { tool: "hello".into(), input: json!({"name": "Ada"}) })
}
