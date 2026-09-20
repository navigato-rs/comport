use std::process::Command;

#[test]
fn prints_version() {
    let output = Command::new(env!("CARGO_BIN_EXE_comport"))
        .output()
        .expect("run comport");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.starts_with("ComPort 0.1.0"),
        "unexpected stdout: {stdout:?}"
    );
}
