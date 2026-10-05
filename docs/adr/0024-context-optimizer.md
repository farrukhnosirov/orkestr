# ADR-0024: Context optimizer — regex symbol/import extraction, explainable packing, execution benchmarks

- **Status:** Accepted
- **Date:** 2026-10-05
- **Phase:** 2
- **Spec:** §20 (Context Optimizer), §22 (Model Benchmarking, Agent Performance), §57 (handoff), §68.38–40
- **Supersedes:** the planned "symbol extraction with tree-sitter" topic in the ADR index

## Context

The context optimizer must pick relevant files, symbols, dependencies, rules,
memory and a concise handoff for a task without sending the whole repository,
and explain each choice. The ADR index planned tree-sitter for symbol
extraction. Model benchmarks and agent analytics need per-execution quality
signals that until Phase 2 were not captured anywhere.

## Decision

1. **Line-oriented regex extraction instead of tree-sitter (for now).**
   `orkestr-context::symbols` and `::imports` extract definitions and import
   specifiers (Rust `use`/`mod`, TS/JS `import`/`export from`/`require`/dynamic
   `import()`, Python `import`/`from`, Go `import`, C# `using` + namespaces)
   with regexes; imports are resolved against the file index into a
   deterministic `ImportGraph`. A per-file identifier set is the symbol
   reference index. Rationale: no native grammar crates (MSVC build cost on the
   Windows-first host, ~10 grammars × compile time), deterministic output, and
   good enough recall for ranking. The extraction functions are the only seam,
   so a tree-sitter backend can replace them later without touching ranking or
   packing.
2. **One pipeline, explainable.** `optimize()` returns a `ContextPlan`: the
   packed bundle plus every considered file / rule / memory note with
   structured `Reason`s and a `Decision` (whole, excerpt, included, over
   budget, unreadable, below cutoff, duplicate). The agent runtime's initial
   message and the `context.preview` / `context.explain` RPCs use the same
   assembly (`orkestr-agent-runtime::context_plan`), so the preview is exactly
   what an agent receives.
3. **Signals.** BM25 seeds + explicit mentions + focus paths (files the task
   touched) → import neighbours and symbol references of the strongest seeds
   (capped at 60 % of the best BM25 score, so neighbours never outrank the
   files that define the work) → git churn as a multiplier (never adds a file
   on its own). Rules: project > global by resolution key (the overridden
   global rule is reported as `Duplicate`). Memory: normalized FTS relevance ×
   kind weight (mistakes / known issues highest) + tag matches.
4. **Model-aware budget.** `budget = min(cap, 0.85 × (window − reserved output
   − system prompt))`; the window comes from the provider model list /
   descriptor, unknown windows use the cap.
5. **Excerpts.** Files above 600 tokens are cut to merged windows (3 lines
   before, 12 after) around matched / referenced symbols.
6. **Handoff compression.** All handoffs of a task are compressed to ≤ 20 % of
   the budget: by the model routed for the `Summarize` purpose when a routing
   rule configures one (45 s timeout, usage recorded), else deterministically
   (goal + recent entries verbatim + headlines). Previews never call a model.
7. **Index cache.** A process-wide LRU cache of file index + import graph per
   root; `FileChanged` marks paths dirty (incremental re-index), roots nobody
   watches expire after 120 s.
8. **Benchmarks from events, exactly once.** The cost engine owns
   `benchmark_executions` (one row per agent instance) and an event cursor.
   The core collector replays the persistent log from the cursor on start and
   then applies live events; usage (tokens, cost, latency, calls) is joined
   from `usage_records` at query time. Post-merge bugs are bug tasks tagged
   `bug:<taskId>` (fits the 40-character tag limit) or children tagged `bug`.
9. **Router auto-scoring.** `orkestr-providers::history::ModelStatsSource` is
   implemented by the core over the benchmark snapshot (per model and per
   model + role/purpose, refreshed after executions finish and every 5 min);
   history adjusts the auto-routing score by at most ±0.4, weighted by sample
   count (ignored below 3 samples).

## Consequences

- Symbol recall is lower than a real parser for exotic syntax (macros,
  multi-line generics); ranking tolerates it, and the seam is isolated.
- Benchmarks start counting from the first event the collector sees; history
  from before the migration is not reconstructed.
- The `bug:<taskId>` tag convention must be used by whoever files bug tasks
  (merge manager, QA agents, users) for post-merge bug attribution.
