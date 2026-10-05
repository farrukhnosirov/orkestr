/**
 * Theme engine (§29, ADR-0015) — public surface of `@orkestr/ui`.
 *
 * - `theme/grammar`  token grammars + strict CSS color parser (mirror of Rust)
 * - `theme/schema`   theme / UI-config validation, normalisation, theme files
 * - `theme/contrast` WCAG contrast report (mirror of Rust)
 * - `theme/color`    color math for the editor (HSV, derived accent tokens)
 * - `theme/engine`   built-ins, effective visuals per Performance Mode, apply
 *
 * Rust (`orkestr_protocol::ui_theme`) and TS are tested against the same
 * fixtures (`crates/protocol/fixtures/ui_theme/cases.json`).
 */
export * from "./theme/grammar";
export * from "./theme/schema";
export * from "./theme/contrast";
export * from "./theme/color";
export * from "./theme/engine";
