//! The command line a compilation unit wraps a recipe's script in.
//!
//! Two paths build it: the sink, as it declares each rule, and the launch of
//! an edge whose recipe was expanded only when it was about to run. They must
//! produce the same bytes for the same script, so what that is gets pinned
//! here rather than inferred from a build's output.

use super::sink::CommandLayout;
use std::path::PathBuf;

fn layout() -> CommandLayout {
    CommandLayout::new(PathBuf::new(), Vec::new(), PathBuf::new(), true)
}

#[test]
fn an_inline_script_is_quoted() {
    let launched = layout().launch(b"/bin/sh", b"-c", b"echo hi", b"out", &[]);
    assert_eq!(launched.command, b"/bin/sh -c \"echo hi\"".to_vec());
    assert!(launched.response_file.is_none());
}

#[test]
fn directory_and_env_prefix_it() {
    let layout = CommandLayout::new(
        PathBuf::from("sub dir"),
        vec![
            (b"KEEP".to_vec(), Some(b"value".to_vec())),
            (b"DROP".to_vec(), None),
        ],
        PathBuf::new(),
        false,
    );
    let launched = layout.launch(b"/bin/sh", b"-c", b"true", b"out", &[]);
    assert_eq!(
        String::from_utf8(launched.command).expect("ascii"),
        "cd 'sub dir' && env -u 'DROP' 'KEEP=value' /bin/sh -c \"true\""
    );
}

#[test]
fn a_long_script_becomes_a_file() {
    let script = vec![b'x'; 100 * 1000 + 1];
    let launched = layout().launch(b"/bin/sh", b"-c", &script, b"out", &[]);
    assert_eq!(launched.command, b"/bin/sh out.rsp".to_vec());
    let (path, content) = launched.response_file.expect("a response file");
    assert_eq!(path, b"out.rsp".to_vec());
    assert_eq!(content, script);
    // The flags belong to a `-c` and a string, and there is neither here.
    assert!(!launched.command.ends_with(b"-c"));
}

#[test]
fn an_awkward_rsp_path_is_quoted() {
    let script = vec![b'x'; 100 * 1000 + 1];
    let launched = layout().launch(b"/bin/sh", b"-c", &script, b"out dir/a b", &[]);
    assert_eq!(
        String::from_utf8(launched.command).expect("ascii"),
        "/bin/sh 'out dir/a b.rsp'"
    );
    let (path, _) = launched.response_file.expect("a response file");
    // The file is written by this build rather than by a shell, so the path it
    // is written to keeps the name the graph holds.
    assert_eq!(path, b"out dir/a b.rsp".to_vec());
}

#[test]
fn a_child_rsp_is_rooted() {
    let layout = CommandLayout::new(
        PathBuf::from("sub"),
        Vec::new(),
        PathBuf::from("/root"),
        false,
    );
    let script = vec![b'x'; 100 * 1000 + 1];
    let launched = layout.launch(b"/bin/sh", b"-c", &script, b"out", &[]);
    assert_eq!(
        String::from_utf8(launched.command).expect("ascii"),
        "cd 'sub' && /bin/sh /root/out.rsp"
    );
    let (path, _) = launched.response_file.expect("a response file");
    assert_eq!(path, b"/root/out.rsp".to_vec());
}

#[test]
fn a_quoted_env_value_survives() {
    let layout = CommandLayout::new(
        PathBuf::new(),
        vec![(b"Q".to_vec(), Some(b"it's".to_vec()))],
        PathBuf::new(),
        true,
    );
    let launched = layout.launch(b"/bin/sh", b"-c", b"true", b"out", &[]);
    assert_eq!(
        String::from_utf8(launched.command).expect("ascii"),
        "env 'Q=it'\\''s' /bin/sh -c \"true\""
    );
}
