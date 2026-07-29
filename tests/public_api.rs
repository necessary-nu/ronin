use ronin::{run, run_os, ErrorKind};
use std::error::Error as _;
use std::ffi::OsString;

#[test]
fn public_api_classifies_cli_errors() {
    let error = run(&[
        "ronin".to_owned(),
        "-d".to_owned(),
        "not-a-debug-mode".to_owned(),
    ])
    .unwrap_err();

    assert_eq!(error.kind(), ErrorKind::Cli);
    assert_eq!(error.to_string(), "unknown debug flag 'not-a-debug-mode'");
    assert!(error.source().is_none());
}

#[test]
fn public_api_preserves_manifest_io_causes() {
    let missing_manifest = std::env::temp_dir().join(format!(
        "ronin-missing-manifest-{}-{}.ninja",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let error = run_os(&[
        OsString::from("ronin"),
        OsString::from("-f"),
        missing_manifest.into_os_string(),
    ])
    .unwrap_err();

    assert_eq!(error.kind(), ErrorKind::Manifest);
    assert!(error.source().is_some());
    assert_eq!(
        error.source().unwrap().to_string(),
        error.to_string(),
        "Ninja-facing text should remain the underlying I/O diagnostic"
    );
}
