//! The command line a compilation unit wraps a recipe's script in.
//!
//! Two paths build it: the sink, as it declares each rule, and the launch of
//! an edge whose recipe was expanded only when it was about to run. They must
//! produce the same bytes for the same script, so what that is gets pinned
//! here rather than inferred from a build's output.
//!
//! Every one of them replaces the launching shell rather than running under
//! it, which is what leaves one process where GNU Make has one and lets a
//! recipe's death by a signal be seen as one.

use super::sink::CommandLayout;
use std::path::PathBuf;

fn layout() -> CommandLayout {
    CommandLayout::new(PathBuf::new(), Vec::new(), PathBuf::new(), true)
}

#[test]
fn an_inline_script_is_quoted() {
    let launched = layout().launch(b"/bin/sh", b"-c", b"echo hi", b"out", &[]);
    assert_eq!(launched.command, b"exec /bin/sh -c \"echo hi\"".to_vec());
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
        "cd 'sub dir' && exec env -u 'DROP' 'KEEP=value' /bin/sh -c \"true\""
    );
}

#[test]
fn a_long_script_becomes_a_file() {
    let script = vec![b'x'; 100 * 1000 + 1];
    let launched = layout().launch(b"/bin/sh", b"-c", &script, b"out", &[]);
    assert_eq!(launched.command, b"exec /bin/sh out.rsp".to_vec());
    let (path, content) = launched.response_file.expect("a response file");
    assert_eq!(path, b"out.rsp".to_vec());
    assert_eq!(content, script);
    // `-c` says the next word is the command and the next word is a file name,
    // so that letter comes off — and a plain recipe's flags are nothing else,
    // so nothing is left to write.
    assert!(!launched.command.ends_with(b"-c"));
}

/// A `.POSIX:` recipe's strictness is the shell's `-e` and not something the
/// script says for itself, so the launch that hands the shell a file has to
/// carry it. It used to write the flags away entirely, which made the length
/// of a recipe decide whether it stopped at its first failure.
#[test]
fn a_long_posix_script_keeps_errexit() {
    let script = vec![b'x'; 100 * 1000 + 1];
    let launched = layout().launch(b"/bin/sh", b"-ec", &script, b"out", &[]);
    assert_eq!(
        String::from_utf8(launched.command).expect("ascii"),
        "exec /bin/sh -e out.rsp"
    );
}

/// Flags a Makefile wrote for itself are not only `-e`, and the same rule
/// carries all of them: the `c` comes off whichever cluster holds it, a word
/// that is an option's argument is left alone, and everything else is copied.
#[test]
fn a_long_script_keeps_makefile_flags() {
    let script = vec![b'x'; 100 * 1000 + 1];
    let launched = layout().launch(b"/bin/bash", b"-o pipefail -xec", &script, b"out", &[]);
    assert_eq!(
        String::from_utf8(launched.command).expect("ascii"),
        "exec /bin/bash -o pipefail -xe out.rsp"
    );
}

#[test]
fn an_awkward_rsp_path_is_quoted() {
    let script = vec![b'x'; 100 * 1000 + 1];
    let launched = layout().launch(b"/bin/sh", b"-c", &script, b"out dir/a b", &[]);
    assert_eq!(
        String::from_utf8(launched.command).expect("ascii"),
        "exec /bin/sh 'out dir/a b.rsp'"
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
        "cd 'sub' && exec /bin/sh /root/out.rsp"
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
        "exec env 'Q=it'\\''s' /bin/sh -c \"true\""
    );
}
