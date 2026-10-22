//! Worker-side job host: accepts `jobs.offer`, verifies the signed policy
//! snapshot, materializes `ORKESTR_HOME/jobs/<id>`, runs the job through an
//! `orkestr_node::JobExecutor`, publishes `JobProgress` on topic `jobs`
//! (Phase 6). **Owner: WP-C.**
