/**
 * Widget config schemas: the same fail-closed JSON-schema subset core
 * enforces on `dashboards.save` (`dashboards_store::config_schema`), plus the
 * form model the Dashboard Builder generates from it.
 */

type Json = unknown;
type Schema = Record<string, unknown>;

const VALIDATION = new Set([
  "type", "properties", "required", "additionalProperties", "enum", "const", "minimum", "maximum", "minLength",
  "maxLength", "pattern", "items", "minItems", "maxItems",
]);
const ANNOTATIONS = new Set(["title", "description", "default", "examples", "format", "$schema", "$id", "readOnly", "writeOnly", "deprecated"]);

export interface SchemaIssue {
  path: string;
  /** i18n key suffix under `dashboards:config.errors.*`. */
  code: "type" | "required" | "enum" | "const" | "minimum" | "maximum" | "minLength" | "maxLength" | "pattern" | "minItems" | "maxItems" | "unknown" | "unsupported" | "noConfig";
  detail?: string;
}

function isObj(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

function typeOk(t: string, v: Json): boolean | null {
  switch (t) {
    case "object":
      return isObj(v);
    case "array":
      return Array.isArray(v);
    case "string":
      return typeof v === "string";
    case "boolean":
      return typeof v === "boolean";
    case "null":
      return v === null;
    case "number":
      return typeof v === "number" && Number.isFinite(v);
    case "integer":
      return typeof v === "number" && Number.isInteger(v);
    default:
      return null;
  }
}

function deepEqual(a: unknown, b: unknown): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

function check(schema: unknown, v: Json, path: string, depth: number, out: SchemaIssue[]): void {
  if (depth > 16) {
    out.push({ path, code: "unsupported", detail: "depth" });
    return;
  }
  if (schema === true) return;
  if (schema === false) {
    out.push({ path, code: "unknown" });
    return;
  }
  if (!isObj(schema)) {
    out.push({ path, code: "unsupported" });
    return;
  }
  for (const k of Object.keys(schema)) {
    if (!VALIDATION.has(k) && !ANNOTATIONS.has(k) && !k.startsWith("x-")) {
      out.push({ path, code: "unsupported", detail: k });
      return;
    }
  }
  const t = schema["type"];
  if (t !== undefined) {
    const types = Array.isArray(t) ? t : [t];
    const results = types.map((x) => (typeof x === "string" ? typeOk(x, v) : null));
    if (results.some((r) => r === null)) {
      out.push({ path, code: "unsupported", detail: String(t) });
      return;
    }
    if (!results.some(Boolean)) {
      out.push({ path, code: "type", detail: types.join("|") });
      return;
    }
  }
  if (Array.isArray(schema["enum"]) && !schema["enum"].some((e) => deepEqual(e, v))) out.push({ path, code: "enum" });
  if ("const" in schema && !deepEqual(schema["const"], v)) out.push({ path, code: "const" });
  if (typeof v === "number") {
    if (typeof schema["minimum"] === "number" && v < schema["minimum"]) out.push({ path, code: "minimum", detail: String(schema["minimum"]) });
    if (typeof schema["maximum"] === "number" && v > schema["maximum"]) out.push({ path, code: "maximum", detail: String(schema["maximum"]) });
  }
  if (typeof v === "string") {
    const len = [...v].length;
    if (typeof schema["minLength"] === "number" && len < schema["minLength"]) out.push({ path, code: "minLength", detail: String(schema["minLength"]) });
    if (typeof schema["maxLength"] === "number" && len > schema["maxLength"]) out.push({ path, code: "maxLength", detail: String(schema["maxLength"]) });
    if (typeof schema["pattern"] === "string") {
      let re: RegExp | null = null;
      try {
        re = new RegExp(schema["pattern"], "u");
      } catch {
        out.push({ path, code: "unsupported", detail: "pattern" });
      }
      if (re && !re.test(v)) out.push({ path, code: "pattern" });
    }
  }
  if (Array.isArray(v)) {
    if (typeof schema["minItems"] === "number" && v.length < schema["minItems"]) out.push({ path, code: "minItems", detail: String(schema["minItems"]) });
    if (typeof schema["maxItems"] === "number" && v.length > schema["maxItems"]) out.push({ path, code: "maxItems", detail: String(schema["maxItems"]) });
    if (schema["items"] !== undefined) v.forEach((item, i) => check(schema["items"], item, `${path}/${i}`, depth + 1, out));
  }
  if (isObj(v)) {
    const props = isObj(schema["properties"]) ? schema["properties"] : {};
    if (Array.isArray(schema["required"])) {
      for (const r of schema["required"]) if (typeof r === "string" && !(r in v)) out.push({ path: `${path}/${r}`, code: "required" });
    }
    for (const [k, val] of Object.entries(v)) {
      if (k in props) check(props[k], val, `${path}/${k}`, depth + 1, out);
      else if (schema["additionalProperties"] === false) out.push({ path: `${path}/${k}`, code: "unknown" });
      else if (isObj(schema["additionalProperties"])) check(schema["additionalProperties"], val, `${path}/${k}`, depth + 1, out);
    }
  }
}

/** Validates a config value; `null`/absent schema accepts only `{}` / `null`. */
export function validateConfig(schema: unknown, value: Json): SchemaIssue[] {
  if (schema === null || schema === undefined) {
    return value === null || (isObj(value) && Object.keys(value).length === 0) ? [] : [{ path: "", code: "noConfig" }];
  }
  const out: SchemaIssue[] = [];
  check(schema, value, "", 0, out);
  return out;
}

// ── form model ───────────────────────────────────────────────────────────

export type FieldKind = "text" | "textarea" | "number" | "integer" | "boolean" | "select" | "json";

export interface ConfigField {
  key: string;
  kind: FieldKind;
  label: string;
  description?: string;
  required: boolean;
  options?: readonly unknown[];
  min?: number;
  max?: number;
  maxLength?: number;
  default?: unknown;
}

/** Form fields of an object schema (top-level properties; nested values as JSON). */
export function configFields(schema: unknown): ConfigField[] {
  if (!isObj(schema) || !isObj(schema["properties"])) return [];
  const required = new Set(Array.isArray(schema["required"]) ? schema["required"].filter((r): r is string => typeof r === "string") : []);
  return Object.entries(schema["properties"]).map(([key, raw]) => {
    const p: Schema = isObj(raw) ? raw : {};
    const t = Array.isArray(p["type"]) ? p["type"].find((x) => x !== "null") : p["type"];
    let kind: FieldKind = "json";
    if (Array.isArray(p["enum"])) kind = "select";
    else if (t === "string") kind = typeof p["maxLength"] === "number" && p["maxLength"] <= 200 ? "text" : "textarea";
    else if (t === "number") kind = "number";
    else if (t === "integer") kind = "integer";
    else if (t === "boolean") kind = "boolean";
    const field: ConfigField = {
      key,
      kind,
      label: typeof p["title"] === "string" ? p["title"] : key,
      required: required.has(key),
    };
    if (typeof p["description"] === "string") field.description = p["description"];
    if (Array.isArray(p["enum"])) field.options = p["enum"];
    if (typeof p["minimum"] === "number") field.min = p["minimum"];
    if (typeof p["maximum"] === "number") field.max = p["maximum"];
    if (typeof p["maxLength"] === "number") field.maxLength = p["maxLength"];
    if ("default" in p) field.default = p["default"];
    return field;
  });
}

/** Initial config for a new instance: schema defaults of top-level properties. */
export function defaultConfig(schema: unknown): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const f of configFields(schema)) if (f.default !== undefined) out[f.key] = f.default;
  return out;
}
