/**
 * Dashboard Builder (Phase 7, ADR-0034). **Owner: WP3.**
 *
 * Grid editor, widget palette, config forms from `configSchema`, the
 * `WidgetFrame` iframe bridge (`@orkestr/widget-sdk` protocol) and native
 * built-in widgets.
 */
export { DashboardBuilder } from "./DashboardBuilder";
export { WidgetFrame } from "./WidgetFrame";
export { createWidgetBridge, parseWidgetMessage, type BridgeClient, type WidgetBridge } from "./bridge";
export { bindingViolation, isBindableMethod } from "./policy";
export { buildWidgetDocument, setWidgetDocumentLoader, WIDGET_CSP, WIDGET_SANDBOX, type WidgetDocumentLoader } from "./frame";
export { configFields, defaultConfig, validateConfig } from "./configSchema";
export * from "./grid";
