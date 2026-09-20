//! Open the real window, paint, close. GPU teardown must not leak Blade blocks.

use std::io;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

fn skip(reason: &str) {
    eprintln!("gpu_lifecycle skip: {reason}");
}

fn spawn_demo() -> Command {
    let binary = env!("CARGO_BIN_EXE_comport");
    let headless =
        std::env::var_os("WAYLAND_DISPLAY").is_none() && std::env::var_os("DISPLAY").is_none();
    let mut cmd = if headless && Path::new("/usr/bin/xvfb-run").exists() {
        let mut cmd = Command::new("xvfb-run");
        cmd.args(["-a", "-s", "-screen 0 1280x800x24", binary]);
        cmd
    } else {
        Command::new(binary)
    };
    cmd.args(["--demo", "--exit-after-frames", "2"]);
    cmd.env("RUST_LOG", "error");
    let lavapipe = Path::new("/usr/share/vulkan/icd.d/lvp_icd.json");
    let icd_set = std::env::var_os("VK_ICD_FILENAMES").is_some_and(|v| !v.is_empty());
    if lavapipe.exists() && !icd_set {
        cmd.env("VK_ICD_FILENAMES", lavapipe);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

#[test]
fn demo_window_open_close_does_not_leak_gpu() {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let result = spawn_demo().output();
        let _ = tx.send(result);
    });
    let output = match rx.recv_timeout(Duration::from_secs(20)) {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => panic!("failed to spawn comport: {error}"),
        Err(_) => panic!("comport --demo --exit-after-frames 2 hung (did not exit in 20s)"),
    };
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{}\n{}", String::from_utf8_lossy(&output.stdout), stderr);
    if combined.contains("GPU initialization failed")
        || combined.contains("create window")
        || combined.contains("GPU surface creation failed")
        || combined.contains("Failed to initialize GPU")
        || combined.contains("WAYLAND_DISPLAY")
        || combined.contains("DISPLAY is set")
        || combined.contains("libxkbcommon")
    {
        skip(&format!("no usable GPU/display:\n{combined}"));
        return;
    }
    if !output.status.success() {
        panic!("comport exited {:?}\n{combined}", output.status);
    }
    assert!(
        !stderr.contains("Leaked GPU"),
        "GPU memory leaked on close:\n{stderr}"
    );
    assert!(
        !stderr.contains("leaked objects"),
        "Vulkan objects leaked on close:\n{stderr}"
    );
    assert!(
        !stderr.contains("vkDestroyDevice"),
        "vkDestroyDevice validation on close:\n{stderr}"
    );
    let _ = io::Write::write_all(
        &mut io::stderr(),
        format!("gpu_lifecycle ok frames=2 stderr_len={}\n", stderr.len()).as_bytes(),
    );
}
