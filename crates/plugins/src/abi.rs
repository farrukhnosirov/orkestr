//! Plugin ABI v1 (ADR-0034). JSON in, JSON out; pointers into guest memory.

/// ABI version (mirrors [`orkestr_domain::PLUGIN_ABI_VERSION`]).
pub const ABI_VERSION: u32 = orkestr_domain::PLUGIN_ABI_VERSION;
/// Guest export: `orkestr_alloc(len: i32) -> ptr: i32`.
pub const EXPORT_ALLOC: &str = "orkestr_alloc";
/// Guest export: `orkestr_call(ptr: i32, len: i32) -> i64` (`ptr << 32 | len` of the JSON result).
pub const EXPORT_CALL: &str = "orkestr_call";
/// Optional guest export (ABI v1, additive): `orkestr_on_event(ptr: i32, len: i32) -> i64`.
/// Receives `{"event": <redacted Event envelope>}` for each `subscribes` kind
/// the plugin was granted (`Read` scope `event/<Kind>`). Required when the
/// manifest declares `subscribes`.
pub const EXPORT_ON_EVENT: &str = "orkestr_on_event";
/// Optional guest export (ABI v1, additive): `orkestr_deploy(ptr: i32, len: i32) -> i64`.
/// Receives `{"op": "availability"|"plan"|"deploy"|"rollback", "provider", "request"}`.
/// Required when the manifest declares `deployProviders`.
pub const EXPORT_DEPLOY: &str = "orkestr_deploy";
/// Host import module.
pub const IMPORT_MODULE: &str = "orkestr";
/// Host import: `host_call(ptr: i32, len: i32) -> i64` (`ptr << 32 | len` of the JSON reply).
pub const IMPORT_HOST_CALL: &str = "host_call";

/// Packs a guest `(ptr, len)` pair into the ABI's `i64`.
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

#[cfg(test)]
mod tests {
    #[test]
    fn pack_roundtrip() {
        let v = super::pack(0x1234, 99);
        assert_eq!(super::unpack(v), (0x1234, 99));
        assert_eq!(super::unpack(super::pack(u32::MAX, u32::MAX)), (u32::MAX, u32::MAX));
    }
}
