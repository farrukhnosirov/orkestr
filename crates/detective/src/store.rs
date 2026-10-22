//! Persistence. Tables are prefixed `detective_`; migrations are composed by
//! `orkestr-core::migrations::all()` (never reorder, never edit an applied one).

use orkestr_storage::Migration;

/// Migrations owned by this crate (owner `detective`). Empty in the step-0
/// skeleton; the owning work package appends `detective_0001_*`.
pub const MIGRATIONS: &[Migration] = &[];

/// [`MIGRATIONS`] as a vector.
#[must_use]
pub fn migrations() -> Vec<Migration> {
    MIGRATIONS.to_vec()
}
