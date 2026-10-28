use std::process::Command;

#[test]
fn version_flag_prints_the_version_and_exits_zero() {
    let output = Command::new(env!("CARGO_BIN_EXE_epik-app"))
        .arg("--version")
        .output()
        .expect("epik-app runs");
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("epik-app {}\n", epik_core::version())
    );
}
