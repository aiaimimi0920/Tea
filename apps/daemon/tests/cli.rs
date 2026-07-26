use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

fn run_daemon_arg(arg: &str) -> (bool, Option<i32>, String, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_tea-daemon"))
        .arg(arg)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn tea-daemon");

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait().expect("poll tea-daemon") {
            let output = child.wait_with_output().expect("collect tea-daemon output");
            return (
                true,
                status.code(),
                String::from_utf8_lossy(&output.stdout).to_string(),
                String::from_utf8_lossy(&output.stderr).to_string(),
            );
        }
        thread::sleep(Duration::from_millis(50));
    }

    child.kill().expect("kill hung tea-daemon");
    let output = child.wait_with_output().expect("collect killed output");
    (
        false,
        None,
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

#[test]
fn help_exits_without_starting_server() {
    let (exited, code, stdout, stderr) = run_daemon_arg("--help");

    assert!(
        exited,
        "tea-daemon --help should exit instead of starting the server; stdout={stdout:?} stderr={stderr:?}"
    );
    assert_eq!(code, Some(0), "stdout={stdout:?} stderr={stderr:?}");
    assert!(stdout.contains("Tea HTTP daemon"), "stdout={stdout:?}");
    assert!(stdout.contains("--bind-addr"), "stdout={stdout:?}");
}

#[test]
fn version_exits_without_starting_server() {
    let (exited, code, stdout, stderr) = run_daemon_arg("--version");

    assert!(
        exited,
        "tea-daemon --version should exit instead of starting the server; stdout={stdout:?} stderr={stderr:?}"
    );
    assert_eq!(code, Some(0), "stdout={stdout:?} stderr={stderr:?}");
    assert!(stdout.contains("tea-daemon"), "stdout={stdout:?}");
}
