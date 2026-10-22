// Greeter module of the Phase 7 acceptance fixture service.

/**
 * Returns a greeting, personalised with an optional display name.
 * @param {string} [name]
 * @param {{ displayName?: string }} [opts]
 * @returns {string}
 */
export function greeter(name = "world", opts) {
  const shown = opts.displayName || name;
  return `Hello, ${shown}!`;
}
