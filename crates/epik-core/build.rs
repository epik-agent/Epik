fn main() {
    // include_crypt! embeds each prompt through a proc macro, which
    // cargo does not watch on its own; this keeps edits to a prompt
    // rebuilding.
    println!("cargo:rerun-if-changed=prompt.md");
    println!("cargo:rerun-if-changed=issue-brief.md");
    println!("cargo:rerun-if-changed=fix-resumed.md");
    println!("cargo:rerun-if-changed=fix-fresh.md");
    println!("cargo:rerun-if-changed=rules.md");
}
