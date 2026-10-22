//! wasmtime sandbox (ADR-0034): no WASI, fuel per call, epoch deadline
//! (timeout + kill switch), `StoreLimits` memory cap. **Owner: WP1.**
//!
//! One [`WasmSandbox`] (= one engine) per core. A ticker thread bumps the
//! engine epoch every [`EPOCH_TICK`]; every store runs with a deadline of one
//! tick and an epoch callback that interrupts the guest when the call's wall
//! clock deadline passed, the call was cancelled or the plugin was killed. So
//! a runaway guest stops within ~one tick of its deadline / the kill switch.
//!
//! Guest code runs on a blocking thread; host calls (`orkestr.host_call`)
//! block that thread on the async [`HostApi`] (bounded by the same deadline
//! and cancellation).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use orkestr_domain::{PluginId, PluginLimits};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;
use wasmtime::{Caller, Engine, ExternType, Linker, Module, Store, StoreLimits, StoreLimitsBuilder, Trap, ValType};

use crate::abi;
use crate::host::{HostApi, HostOp};
use crate::PluginError;

/// Epoch tick.
pub const EPOCH_TICK: Duration = Duration::from_millis(10);
/// Max bytes of one guest payload (call result, host request).
pub const MAX_PAYLOAD_BYTES: u32 = 4 * 1024 * 1024;
/// Max host calls per plugin call.
pub const MAX_HOST_CALLS: u32 = 1_000;

/// Engine configuration shared by every plugin: fuel metering and epoch
/// interruption on; the minimal feature set has no WASI, threads or component model.
#[must_use]
pub fn engine_config() -> wasmtime::Config {
    let mut cfg = wasmtime::Config::new();
    cfg.consume_fuel(true);
    cfg.epoch_interruption(true);
    cfg
}

struct Ticker {
    stop: Arc<AtomicBool>,
}

impl Drop for Ticker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// The plugin sandbox (one wasmtime engine + epoch ticker per core). Cheap to clone.
#[derive(Clone)]
pub struct WasmSandbox {
    engine: Engine,
    _ticker: Arc<Ticker>,
}

impl std::fmt::Debug for WasmSandbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmSandbox").finish_non_exhaustive()
    }
}

/// Cancellation of one call: `killed` (kill switch) or `cancel` (caller gone).
#[derive(Debug, Clone, Default)]
pub struct CallControl {
    /// Set by the kill switch.
    pub killed: Arc<AtomicBool>,
    /// Cancelled when the caller (agent, RPC) goes away.
    pub cancel: CancellationToken,
}

/// Result of one call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallOutput {
    /// Raw JSON bytes returned by `orkestr_call`.
    pub output: Vec<u8>,
    /// Fuel consumed.
    pub fuel_used: u64,
    /// Wall clock.
    pub duration: Duration,
}

struct State {
    limits: StoreLimits,
    plugin: PluginId,
    host: Arc<dyn HostApi>,
    handle: tokio::runtime::Handle,
    ctl: CallControl,
    deadline: Instant,
    host_calls: u32,
}

#[derive(Deserialize)]
struct HostRequest {
    op: HostOp,
    #[serde(default)]
    input: Value,
}

fn is(v: &ValType, want: &ValType) -> bool {
    matches!((v, want), (ValType::I32, ValType::I32) | (ValType::I64, ValType::I64))
}

fn func_sig(t: &ExternType, params: &[ValType], results: &[ValType]) -> bool {
    let ExternType::Func(f) = t else { return false };
    let p: Vec<ValType> = f.params().collect();
    let r: Vec<ValType> = f.results().collect();
    p.len() == params.len()
        && r.len() == results.len()
        && p.iter().zip(params).all(|(a, b)| is(a, b))
        && r.iter().zip(results).all(|(a, b)| is(a, b))
}

fn mem_error(e: &impl std::fmt::Display) -> PluginError {
    PluginError::Trap(format!("bad guest memory access: {e}"))
}

impl WasmSandbox {
    /// Creates the engine and starts its epoch ticker thread.
    ///
    /// # Errors
    /// Engine configuration rejected by wasmtime / thread spawn failure.
    pub fn new() -> Result<Self, PluginError> {
        let engine = Engine::new(&engine_config()).map_err(|e| PluginError::Internal(e.to_string()))?;
        let stop = Arc::new(AtomicBool::new(false));
        let (e2, s2) = (engine.clone(), Arc::clone(&stop));
        std::thread::Builder::new()
            .name("orkestr-plugin-epoch".into())
            .spawn(move || {
                while !s2.load(Ordering::Relaxed) {
                    std::thread::sleep(EPOCH_TICK);
                    e2.increment_epoch();
                }
            })
            .map_err(PluginError::Io)?;
        Ok(Self { engine, _ticker: Arc::new(Ticker { stop }) })
    }

    /// The engine.
    #[must_use]
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Compiles a module and checks ABI v1: exports `memory`,
    /// `orkestr_alloc(i32)->i32`, `orkestr_call(i32,i32)->i64`; the only
    /// allowed import is `orkestr.host_call(i32,i32)->i64` (no WASI).
    ///
    /// # Errors
    /// Invalid wasm / ABI mismatch.
    pub fn compile(&self, wasm: &[u8]) -> Result<Module, PluginError> {
        let module =
            Module::new(&self.engine, wasm).map_err(|e| PluginError::Package(format!("invalid wasm module: {e}")))?;
        for imp in module.imports() {
            let ok = imp.module() == abi::IMPORT_MODULE
                && imp.name() == abi::IMPORT_HOST_CALL
                && func_sig(&imp.ty(), &[ValType::I32, ValType::I32], &[ValType::I64]);
            if !ok {
                return Err(PluginError::Package(format!(
                    "ABI v1: import `{}`.`{}` is not allowed (only `orkestr`.`host_call`)",
                    imp.module(),
                    imp.name()
                )));
            }
        }
        let has = |name: &str, check: &dyn Fn(&ExternType) -> bool| {
            module.exports().any(|e| e.name() == name && check(&e.ty()))
        };
        if !has("memory", &|t| matches!(t, ExternType::Memory(_))) {
            return Err(PluginError::Package("ABI v1: the module must export `memory`".into()));
        }
        if !has(abi::EXPORT_ALLOC, &|t| func_sig(t, &[ValType::I32], &[ValType::I32])) {
            return Err(PluginError::Package("ABI v1: missing export `orkestr_alloc(i32) -> i32`".into()));
        }
        if !has(abi::EXPORT_CALL, &|t| func_sig(t, &[ValType::I32, ValType::I32], &[ValType::I64])) {
            return Err(PluginError::Package("ABI v1: missing export `orkestr_call(i32, i32) -> i64`".into()));
        }
        for name in [abi::EXPORT_ON_EVENT, abi::EXPORT_DEPLOY] {
            let present = module.exports().any(|e| e.name() == name);
            if present && !has(name, &|t| func_sig(t, &[ValType::I32, ValType::I32], &[ValType::I64])) {
                return Err(PluginError::Package(format!("ABI v1: export `{name}` must be `(i32, i32) -> i64`")));
            }
        }
        Ok(module)
    }

    /// Runs one call of `orkestr_call` on a blocking thread (see [`Self::call_blocking`]).
    ///
    /// # Errors
    /// See [`Self::call_blocking`].
    pub async fn call(
        &self,
        module: Module,
        plugin: PluginId,
        input: Vec<u8>,
        limits: PluginLimits,
        host: Arc<dyn HostApi>,
        ctl: CallControl,
    ) -> Result<CallOutput, PluginError> {
        self.call_export(module, abi::EXPORT_CALL, plugin, input, limits, host, ctl).await
    }

    /// Runs one call of the guest export `export` (`orkestr_call`,
    /// `orkestr_on_event`, `orkestr_deploy`) on a blocking thread.
    ///
    /// # Errors
    /// See [`Self::call_blocking`]; a missing export traps.
    #[allow(clippy::too_many_arguments)]
    pub async fn call_export(
        &self,
        module: Module,
        export: &'static str,
        plugin: PluginId,
        input: Vec<u8>,
        limits: PluginLimits,
        host: Arc<dyn HostApi>,
        ctl: CallControl,
    ) -> Result<CallOutput, PluginError> {
        let sb = self.clone();
        let handle = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            sb.call_blocking_export(&module, export, plugin, &input, limits, host, handle, ctl)
        })
        .await
        .map_err(|e| PluginError::Internal(format!("plugin call thread failed: {e}")))?
    }

    /// Instantiates `module` in a fresh store and calls `orkestr_call` with
    /// `input` under `limits`. Must run on a thread that may block (it blocks
    /// on `handle` for host calls).
    ///
    /// # Errors
    /// [`PluginError::Killed`], [`PluginError::Cancelled`] or
    /// [`PluginError::Trap`] (fuel, timeout, memory cap, guest trap, bad ABI use).
    #[allow(clippy::too_many_arguments)]
    pub fn call_blocking(
        &self,
        module: &Module,
        plugin: PluginId,
        input: &[u8],
        limits: PluginLimits,
        host: Arc<dyn HostApi>,
        handle: tokio::runtime::Handle,
        ctl: CallControl,
    ) -> Result<CallOutput, PluginError> {
        self.call_blocking_export(module, abi::EXPORT_CALL, plugin, input, limits, host, handle, ctl)
    }

    /// [`Self::call_blocking`] for any guest export.
    ///
    /// # Errors
    /// See [`Self::call_blocking`].
    #[allow(clippy::too_many_arguments, clippy::needless_pass_by_value)]
    pub fn call_blocking_export(
        &self,
        module: &Module,
        export: &str,
        plugin: PluginId,
        input: &[u8],
        limits: PluginLimits,
        host: Arc<dyn HostApi>,
        handle: tokio::runtime::Handle,
        ctl: CallControl,
    ) -> Result<CallOutput, PluginError> {
        let started = Instant::now();
        let deadline = started + Duration::from_millis(u64::from(limits.timeout_ms));
        let mem_bytes = usize::try_from(limits.memory_mb).unwrap_or(64) * 1024 * 1024;
        let state = State {
            limits: StoreLimitsBuilder::new()
                .memory_size(mem_bytes)
                .instances(1)
                .memories(1)
                .tables(4)
                .table_elements(100_000)
                .trap_on_grow_failure(true)
                .build(),
            plugin,
            host,
            handle,
            ctl: ctl.clone(),
            deadline,
            host_calls: 0,
        };
        let mut store = Store::new(&self.engine, state);
        store.limiter(|s| &mut s.limits);
        store.set_fuel(limits.fuel_per_call).map_err(|e| PluginError::Internal(e.to_string()))?;
        store.set_epoch_deadline(1);
        let cb = ctl.clone();
        store.epoch_deadline_callback(move |_| {
            if cb.killed.load(Ordering::Relaxed) || cb.cancel.is_cancelled() || Instant::now() >= deadline {
                Ok(wasmtime::UpdateDeadline::Interrupt)
            } else {
                Ok(wasmtime::UpdateDeadline::Continue(1))
            }
        });
        let result = run(&self.engine, &mut store, module, export, input);
        let fuel_used = limits.fuel_per_call.saturating_sub(store.get_fuel().unwrap_or(0));
        let classify = |e: PluginError| -> PluginError {
            if ctl.killed.load(Ordering::Relaxed) {
                PluginError::Killed
            } else if ctl.cancel.is_cancelled() {
                PluginError::Cancelled
            } else {
                e
            }
        };
        match result {
            Ok(output) => Ok(CallOutput { output, fuel_used, duration: started.elapsed() }),
            Err(RunError::Plugin(e)) => Err(classify(e)),
            Err(RunError::Wasm(e)) => {
                let msg = format!("{e:#}");
                let err = if e.downcast_ref::<Trap>() == Some(&Trap::OutOfFuel) {
                    PluginError::Trap(format!("fuel exhausted (limit {})", limits.fuel_per_call))
                } else if e.downcast_ref::<Trap>() == Some(&Trap::Interrupt) || Instant::now() >= deadline {
                    PluginError::Trap(format!("timed out after {} ms", limits.timeout_ms))
                } else if msg.contains("memory") && (msg.contains("grow") || msg.contains("limit")) {
                    PluginError::Trap(format!("memory limit of {} MiB exceeded", limits.memory_mb))
                } else {
                    PluginError::Trap(msg.chars().take(500).collect())
                };
                Err(classify(err))
            }
        }
    }
}

enum RunError {
    Wasm(wasmtime::Error),
    Plugin(PluginError),
}

impl From<wasmtime::Error> for RunError {
    fn from(e: wasmtime::Error) -> Self {
        RunError::Wasm(e)
    }
}

fn run(
    engine: &Engine,
    store: &mut Store<State>,
    module: &Module,
    export: &str,
    input: &[u8],
) -> Result<Vec<u8>, RunError> {
    let mut linker: Linker<State> = Linker::new(engine);
    linker.func_wrap(abi::IMPORT_MODULE, abi::IMPORT_HOST_CALL, host_call)?;
    let instance = linker.instantiate(&mut *store, module)?;
    let memory = instance
        .get_memory(&mut *store, "memory")
        .ok_or_else(|| RunError::Plugin(PluginError::Trap("no exported memory".into())))?;
    let alloc = instance.get_typed_func::<i32, i32>(&mut *store, abi::EXPORT_ALLOC)?;
    let call = instance.get_typed_func::<(i32, i32), i64>(&mut *store, export)?;
    let len =
        i32::try_from(input.len()).map_err(|_| RunError::Plugin(PluginError::Invalid("input too large".into())))?;
    let ptr = alloc.call(&mut *store, len)?;
    memory
        .write(&mut *store, usize::try_from(ptr.cast_unsigned()).unwrap_or(usize::MAX), input)
        .map_err(|e| RunError::Plugin(mem_error(&e)))?;
    let packed = call.call(&mut *store, (ptr, len))?;
    let (rp, rl) = abi::unpack(packed);
    if rl > MAX_PAYLOAD_BYTES {
        return Err(RunError::Plugin(PluginError::Trap(format!("result larger than {MAX_PAYLOAD_BYTES} bytes"))));
    }
    let mut out = vec![0u8; rl as usize];
    memory.read(&*store, rp as usize, &mut out).map_err(|e| RunError::Plugin(mem_error(&e)))?;
    Ok(out)
}

fn reply_bytes(v: &Value) -> Vec<u8> {
    serde_json::to_vec(v).unwrap_or_else(|_| b"{\"error\":\"unserializable reply\"}".to_vec())
}

fn host_call(mut caller: Caller<'_, State>, ptr: i32, len: i32) -> wasmtime::Result<i64> {
    let memory = caller
        .get_export("memory")
        .and_then(wasmtime::Extern::into_memory)
        .ok_or_else(|| wasmtime::format_err!("plugin has no exported memory"))?;
    let len = len.cast_unsigned();
    if len > MAX_PAYLOAD_BYTES {
        wasmtime::bail!("host request larger than {MAX_PAYLOAD_BYTES} bytes");
    }
    let mut buf = vec![0u8; len as usize];
    memory.read(&caller, ptr.cast_unsigned() as usize, &mut buf)?;
    let state = caller.data_mut();
    state.host_calls += 1;
    if state.host_calls > MAX_HOST_CALLS {
        wasmtime::bail!("more than {MAX_HOST_CALLS} host calls in one plugin call");
    }
    let reply = match serde_json::from_slice::<HostRequest>(&buf) {
        Err(e) => json!({ "error": format!("invalid host request: {e}") }),
        Ok(req) => {
            let (host, plugin, ctl, deadline) =
                (Arc::clone(&state.host), state.plugin, state.ctl.clone(), state.deadline);
            let remaining = deadline.saturating_duration_since(Instant::now());
            let outcome = state.handle.block_on(async move {
                tokio::select! {
                    r = host.call(&plugin, req.op, req.input) => Some(r),
                    () = ctl.cancel.cancelled() => None,
                    () = tokio::time::sleep(remaining) => None,
                }
            });
            match outcome {
                None => wasmtime::bail!("host call interrupted (timeout, cancel or kill)"),
                Some(_) if caller.data().ctl.killed.load(Ordering::Relaxed) => wasmtime::bail!("plugin killed"),
                Some(Ok(v)) => json!({ "ok": v }),
                Some(Err(e)) => json!({ "error": e.to_string() }),
            }
        }
    };
    let out = reply_bytes(&reply);
    let alloc = caller
        .get_export(abi::EXPORT_ALLOC)
        .and_then(wasmtime::Extern::into_func)
        .ok_or_else(|| wasmtime::format_err!("plugin has no orkestr_alloc"))?
        .typed::<i32, i32>(&caller)?;
    let out_len = i32::try_from(out.len())?;
    let p = alloc.call(&mut caller, out_len)?;
    memory.write(&mut caller, p.cast_unsigned() as usize, &out)?;
    Ok(abi::pack(p.cast_unsigned(), out_len.cast_unsigned()))
}

/// Whether `module` exports a function named `name`.
#[must_use]
pub fn has_export(module: &Module, name: &str) -> bool {
    module.exports().any(|e| e.name() == name && matches!(e.ty(), ExternType::Func(_)))
}

/// Parses the guest's JSON result envelope: `{"ok": value}` в†’ `Ok(value)`,
/// `{"error": "msg"}` в†’ `Err(msg)`, anything else в†’ `Ok(whole value)`.
///
/// # Errors
/// The guest reported an error or returned invalid JSON.
pub fn parse_result(bytes: &[u8]) -> Result<Value, PluginError> {
    let v: Value =
        serde_json::from_slice(bytes).map_err(|e| PluginError::Trap(format!("plugin returned invalid JSON: {e}")))?;
    if let Value::Object(map) = &v {
        if let Some(ok) = map.get("ok") {
            return Ok(ok.clone());
        }
        if let Some(err) = map.get("error") {
            let msg = err.as_str().map_or_else(|| err.to_string(), ToOwned::to_owned);
            return Err(PluginError::Invalid(format!("plugin error: {}", msg.chars().take(2_000).collect::<String>())));
        }
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::DenyAllHost;
    use crate::wat;

    fn limits(timeout_ms: u32) -> PluginLimits {
        PluginLimits { timeout_ms, ..PluginLimits::default() }
    }

    async fn run_wat(src: &str, l: PluginLimits, ctl: CallControl) -> Result<CallOutput, PluginError> {
        let sb = WasmSandbox::new().unwrap();
        let m = sb.compile(&wat::to_wasm(&wat::by_name(src).unwrap()).unwrap()).unwrap();
        sb.call(
            m,
            PluginId::new(),
            br#"{"tool":"hello","input":{"name":"Ada"}}"#.to_vec(),
            l,
            Arc::new(DenyAllHost),
            ctl,
        )
        .await
    }

    #[test]
    fn engine_builds() {
        let sb = WasmSandbox::new().unwrap();
        let module = Module::new(sb.engine(), "(module)").unwrap();
        assert_eq!(module.exports().count(), 0);
    }

    #[test]
    fn abi_is_checked() {
        let sb = WasmSandbox::new().unwrap();
        assert!(sb.compile(&::wat::parse_str("(module)").unwrap()).is_err());
        let wasi = r#"(module (import "wasi_snapshot_preview1" "fd_write" (func (param i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (func (export "orkestr_alloc") (param i32) (result i32) i32.const 0)
            (func (export "orkestr_call") (param i32 i32) (result i64) i64.const 0))"#;
        assert!(
            matches!(sb.compile(&::wat::parse_str(wasi).unwrap()), Err(PluginError::Package(m)) if m.contains("not allowed"))
        );
        assert!(sb.compile(&wat::to_wasm(&wat::hello()).unwrap()).is_ok());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hello_echoes_and_reports_fuel() {
        let out = run_wat(wat::HELLO, limits(5_000), CallControl::default()).await.unwrap();
        let v = parse_result(&out.output).unwrap();
        assert_eq!(v["echo"]["tool"], "hello");
        assert!(out.fuel_used > 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn infinite_loop_traps_under_a_second() {
        let t = Instant::now();
        let l = PluginLimits { fuel_per_call: crate::package::MAX_FUEL, ..limits(300) };
        let err = run_wat(wat::LOOP, l, CallControl::default()).await.unwrap_err();
        assert!(t.elapsed() < Duration::from_secs(1), "took {:?}", t.elapsed());
        assert!(matches!(&err, PluginError::Trap(m) if m.contains("timed out")), "{err}");
        // Fuel exhaustion also stops it.
        let l = PluginLimits { fuel_per_call: 1_000_000, ..limits(10_000) };
        let err = run_wat(wat::LOOP, l, CallControl::default()).await.unwrap_err();
        assert!(matches!(&err, PluginError::Trap(m) if m.contains("fuel")), "{err}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn memory_bomb_is_capped() {
        let l = PluginLimits { memory_mb: 16, ..limits(5_000) };
        let err = run_wat(wat::MEMORY_BOMB, l, CallControl::default()).await.unwrap_err();
        assert!(matches!(&err, PluginError::Trap(m) if m.contains("memory")), "{err}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn kill_interrupts_a_running_call() {
        let ctl = CallControl::default();
        let k = ctl.killed.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            k.store(true, Ordering::Relaxed);
        });
        let t = Instant::now();
        let err = run_wat(wat::LOOP, limits(30_000), ctl).await.unwrap_err();
        assert!(matches!(err, PluginError::Killed), "{err}");
        assert!(t.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn host_call_roundtrip_reports_denial() {
        let out = run_wat(wat::FETCH, limits(5_000), CallControl::default()).await.unwrap();
        let v: Value = serde_json::from_slice(&out.output).unwrap();
        assert!(v["error"].as_str().unwrap().contains("denied"), "{v}");
    }
}
