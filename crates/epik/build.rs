fn main() {
    // include_crypt! embeds prompt.md through a proc macro, which cargo
    // does not watch on its own; this keeps edits to the prompt rebuilding.
    println!("cargo:rerun-if-changed=prompt.md");
}
