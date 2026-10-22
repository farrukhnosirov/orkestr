//! Coordinator job driver: place → offer (epoch + 1) → accept → progress
//! intake (stale epochs dropped) → completion through the task engine, which
//! stays the only task-status writer (Phase 6). **Owner: WP-C.**
