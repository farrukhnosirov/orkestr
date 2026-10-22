//! wasmtime sandbox (ADR-0034): no WASI, fuel per call, epoch deadline
//! (timeout), `StoreLimits` memory cap. **Owner: WP1.**

use orkestr_domain::PluginLimits;

use crate::PluginError;

/// Engine configuration shared by every plugin: fuel metering and epoch
/// interruption on; the minimal feature set has no WASI, threads or component model.
#[must_use]
pub fn engine_config() -> wasmtime::Config {
    let mut cfg = wasmtime::Config::new();
    cfg.consume_fuel(true);
    cfg.epoch_interruption(true);
    cfg
}

/// The plugin sandbox (one wasmtime engine per core).
#[derive(Clone)]
pub struct WasmSandbox {
    engine: wasmtime::Engine,
}

impl std::fmt::Debug for WasmSandbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmSandbox").finish_non_exhaustive()
    }
}

impl WasmSandbox {
    /// Creates the engine.
    ///
    /// # Errors
    /// Engine configuration rejected by wasmtime.
    pub fn new() -> Result<Self, PluginError> {
        let engine = wasmtime::Engine::new(&engine_config()).map_err(|e| PluginError::Internal(e.to_string()))?;
        Ok(Self { engine })
    }

    /// The engine.
    #[must_use]
    pub fn engine(&self) -> &wasmtime::Engine {
        &self.engine
    }

    /// Calls `orkestr_call` of `module` with a JSON input under `limits`.
    ///
    /// # Errors
    /// Not implemented in the step-0 skeleton.
    pub fn call(&self, _module: &[u8], _input: &[u8], _limits: PluginLimits) -> Result<Vec<u8>, PluginError> {
        Err(PluginError::Unimplemented("plugins.runtime"))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn engine_builds() {
        let sb = super::WasmSandbox::new().unwrap();
        let module = wasmtime::Module::new(sb.engine(), "(module)").unwrap();
        assert_eq!(module.exports().count(), 0);
    }
}
