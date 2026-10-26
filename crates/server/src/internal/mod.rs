//! Built-in (non-wasm) sources: local directory instances and OPDS catalogs.
//! Registration for both goes through the shared internal-source endpoint
//! (`api/internal_source.rs`, a tagged-union dispatch on `type`); a future
//! built-in kind is one more submodule here plus its registration function.

pub mod local;
pub mod opds_source;
