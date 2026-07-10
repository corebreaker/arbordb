//! Secondary indexes.
//!
//! Built incrementally: this phase lands the order-preserving key encoding
//! ([`ordered`]); index definitions, the persisted registry, write-time
//! maintenance, back-fill, and the query builder follow in later sub-phases.

mod ordered;
