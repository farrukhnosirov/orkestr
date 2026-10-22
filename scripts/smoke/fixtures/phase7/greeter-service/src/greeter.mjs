// Greeter module of the Phase 7 acceptance fixture service.

/**
 * Returns a greeting.
 * @param {string} [name]
 * @returns {string}
 */
export function greeter(name = "world") {
  return `Hello, ${name}!`;
}
