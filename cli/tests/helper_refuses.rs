//! The Helper answers only callers it can name. These start a real Helper on
//! a temporary root and call it from programs it must refuse.
#![cfg(target_os = "macos")]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

struct Helper {
    child: Child,
    root: PathBuf,
}

impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn start(tag: &str) -> Helper {
    let root = std::env::temp_dir().join(format!("oa-helper-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("apps")).unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_openas"))
        .arg("serve")
        .env("OPENAS_ROOT", &root)
        .spawn()
        .unwrap();
    let sock = root.join("helper.sock");
    let t = Instant::now();
    while UnixStream::connect(&sock).is_err() {
        assert!(
            t.elapsed() < Duration::from_secs(10),
            "the Helper never started"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    Helper { child, root }
}

/// Sends one request and returns the final line (`ok` or `error`).
fn ask(sock: &Path, req: &str) -> String {
    let mut s = UnixStream::connect(sock).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    writeln!(s, "{req}").unwrap();
    let mut last = String::new();
    for line in BufReader::new(&s).lines() {
        let Ok(line) = line else { break };
        last = line;
        if last.contains("\"ok\"") || last.contains("\"error\"") {
            break;
        }
    }
    last
}

#[test]
fn a_stranger_is_refused() {
    // This test binary is not OpenAs, its window, or an app it installed.
    let h = start("stranger");
    let reply = ask(&h.root.join("helper.sock"), r#"{"op":"settings"}"#);
    assert!(reply.contains("\"error\""), "{reply}");
    assert!(reply.contains("not an OpenAs app"), "{reply}");
}

#[test]
fn a_modified_copy_of_an_app_is_refused() {
    // An app OpenAs recorded, but the program at that path is not the copy
    // it installed (its code hash differs).
    let h = start("modified");
    let bundle = h.root.join("Plants.app");
    let exe = bundle.join("Contents/MacOS/Plants");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::copy(std::env::current_exe().unwrap(), &exe).unwrap();
    let app = h.root.join("apps/plants");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::write(
        app.join("app.json"),
        serde_json::json!({ "id": "plants", "name": "Plants", "provider": "", "model": "", "created": 1,
                            "installed": bundle, "cdhash": "0000000000000000000000000000000000000000" })
        .to_string(),
    )
    .unwrap();
    // The copy runs only `as_a_client` below, which calls the Helper.
    let out = Command::new(&exe)
        .args(["--ignored", "--exact", "as_a_client", "--nocapture"])
        .env("OA_TEST_SOCK", h.root.join("helper.sock"))
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("REPLY") && text.contains("\"error\""),
        "{text}"
    );
    assert!(text.contains("modified"), "{text}");
}

/// Run by `a_modified_copy_of_an_app_is_refused` from inside a fake app.
#[test]
#[ignore]
fn as_a_client() {
    let sock = std::env::var("OA_TEST_SOCK").expect("only run from the other test");
    println!(
        "REPLY {}",
        ask(Path::new(&sock), r#"{"op":"undo","app":"plants"}"#)
    );
}
