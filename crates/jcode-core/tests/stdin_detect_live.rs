//! Live check that stdin detection sees a process actually blocked on a read.
//!
//! The unit tests around this only exercise the plumbing; nothing asserted that
//! the platform probe reports `Reading` for a real blocked process. It did not
//! on macOS — `TH_STATE_WAITING` was defined as 2 (`TH_STATE_STOPPED`) instead
//! of 3, so the check never matched and every interactive command sat there
//! until it timed out. That is invisible to a mocked test, hence this one.

#![cfg(unix)]

use jcode_core::stdin_detect::{StdinState, is_waiting_for_stdin};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn a_process_blocked_reading_stdin_is_detected() {
    let mut child = Command::new("bash")
        .arg("-c")
        .arg("head -n1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn a child that reads stdin");

    // The child needs a moment to reach the read; poll rather than sleep a
    // fixed amount so this does not get flaky on a loaded machine.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut last = StdinState::Unknown;
    while Instant::now() < deadline {
        last = is_waiting_for_stdin(child.id());
        if last == StdinState::Reading {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let _ = child.kill();
    let _ = child.wait();

    assert_eq!(
        last,
        StdinState::Reading,
        "a child blocked on `head -n1` must be reported as reading stdin"
    );
}

/// The complement: a process that is not reading must not be reported as
/// reading, or every command would prompt for input it never wanted.
#[test]
fn a_process_not_reading_stdin_is_not_detected() {
    let mut child = Command::new("bash")
        .arg("-c")
        .arg("sleep 3")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn a child that ignores stdin");

    std::thread::sleep(Duration::from_millis(500));
    let state = is_waiting_for_stdin(child.id());

    let _ = child.kill();
    let _ = child.wait();

    assert_ne!(
        state,
        StdinState::Reading,
        "a sleeping child must not be reported as reading stdin"
    );
}
