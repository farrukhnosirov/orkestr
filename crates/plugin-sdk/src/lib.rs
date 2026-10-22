//! Orkestr plugin SDK, guest side (ADR-0034). **Owner: WP1.**
//!
//! Build a plugin with `cargo build --target wasm32-unknown-unknown --release`
//! (see `examples/plugins/`). ABI v1: the module exports
//! `orkestr_alloc(len) -> ptr` and `orkestr_call(ptr, len) -> i64`
//! (`ptr << 32 | len` of the JSON result) and imports
//! `"orkestr" "host_call"(ptr, len) -> i64`.
//!
//! On non-wasm targets only the target-independent helpers compile; the
//! FFI glue (`#[cfg(target_arch = "wasm32")]`) is added by WP1.

#![cfg_attr(not(target_arch = "wasm32"), forbid(unsafe_code))]

/// ABI version this SDK implements.
pub const ABI_VERSION: u32 = 1;

/// Packs `(ptr, len)` into the ABI's `i64`.
#[must_use]
pub fn pack(ptr: u32, len: u32) -> i64 {
    ((u64::from(ptr) << 32) | u64::from(len)).cast_signed()
}

/// Unpacks the ABI's `i64` into `(ptr, len)`.
#[must_use]
pub fn unpack(v: i64) -> (u32, u32) {
    let v = v.cast_unsigned();
    ((v >> 32) as u32, (v & 0xffff_ffff) as u32)
}

/// A tool call request delivered to `orkestr_call`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CallRequest {
    /// Tool name.
    pub tool: String,
    /// JSON input.
    pub input: serde_json::Value,
}

#[cfg(test)]
mod tests {
    #[test]
    fn pack_roundtrip() {
        assert_eq!(super::unpack(super::pack(7, 9)), (7, 9));
    }
}
