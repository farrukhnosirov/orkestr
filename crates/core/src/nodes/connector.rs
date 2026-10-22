//! Gateway connector: coordinator → worker E2E links over `/v1/connect`
//! with a core bearer token (same account), signed with the coordinator CIK
//! (Phase 6, ADR-0033). **Owner: WP-A.**

/// `orkestr_node::NodeConnector` over the gateway (Step 0 skeleton; WP-A implements).
#[derive(Debug, Default)]
pub struct GatewayConnector;
