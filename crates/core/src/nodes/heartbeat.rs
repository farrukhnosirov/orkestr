//! Pull heartbeat loop: the coordinator calls `node.heartbeat` on every worker
//! (`ORKESTR_NODE_HEARTBEAT_MS`, default 5 s), records samples / leases and
//! drives the health state machine (Phase 6). **Owner: WP-A.**
