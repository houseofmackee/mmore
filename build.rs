//! Builds the syntax bundle: syntect's built-in syntaxes plus the extra grammars in
//! `assets/syntaxes`, precompiled so startup does not parse YAML. It is stored
//! uncompressed: loading is then about 20 times faster, for a slightly larger binary.

use std::env;
use std::path::PathBuf;

use syntect::dumps::dump_to_uncompressed_file;
use syntect::parsing::SyntaxSet;

fn main() {
    println!("cargo:rerun-if-changed=assets/syntaxes");

    let mut builder = SyntaxSet::load_defaults_newlines().into_builder();
    builder
        .add_from_folder("assets/syntaxes", true)
        .expect("failed to load assets/syntaxes");
    let syntaxes = builder.build();

    let out =
        PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not set")).join("syntaxes.packdump");
    dump_to_uncompressed_file(&syntaxes, &out).expect("failed to write the syntax bundle");
}
