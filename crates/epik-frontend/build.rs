//! Compiles the vendored grammars into the pruned SyntaxSet dump the
//! highlighter embeds. Regeneration is just `cargo build` — the set is
//! exactly what sits in `grammars/`; see its README for provenance.

use std::path::Path;

use syntect::parsing::SyntaxSetBuilder;

fn main() {
    println!("cargo:rerun-if-changed=grammars");
    let mut builder = SyntaxSetBuilder::new();
    builder
        .add_from_folder("grammars", true)
        .expect("every vendored grammar loads");
    let set = builder.build();
    let dump = syntect::dumps::dump_binary(&set);
    let out = std::env::var("OUT_DIR").expect("cargo sets OUT_DIR");
    std::fs::write(Path::new(&out).join("syntaxes.packdump"), dump)
        .expect("the dump writes into OUT_DIR");
}
