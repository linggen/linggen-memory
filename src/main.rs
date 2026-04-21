//! `ling-mem` binary entry.
//!
//! v0.1 scope: a minimal placeholder that proves the binary links. Real CLI
//! subcommands land incrementally in follow-up commits (see `doc/tech-spec.md`
//! for the planned `add / get / search / list / update / delete / forget`
//! contract).

fn main() {
    eprintln!(
        "ling-mem {} — v0.1 in progress.\n\
         CLI subcommands land incrementally; see doc/tech-spec.md for the plan.",
        env!("CARGO_PKG_VERSION"),
    );
}
