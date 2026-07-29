use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEST: AtomicUsize = AtomicUsize::new(0);

fn test_directory(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "ronin-{label}-{}-{}",
        std::process::id(),
        NEXT_TEST.fetch_add(1, Ordering::Relaxed)
    ))
}

// [spec:samurai:req:product.ronin-identity/test]
// [spec:samurai:req:product.no-samuflags/test]
// [spec:samurai:req:compat.version-reporting/test]
// [spec:samurai:sem:samu.main-fn+1/test]
// [spec:samurai:sem:samu.parseenvargs-fn+1/test]
#[test]
fn binary_is_ronin_and_ignores_samuflags() {
    let binary = env!("CARGO_BIN_EXE_ronin");
    assert!(
        PathBuf::from(binary)
            .file_stem()
            .is_some_and(|name| name == "ronin"),
        "unexpected binary path: {binary}"
    );

    let output = Command::new(binary)
        .arg("--version")
        .env("SAMUFLAGS", "-d invalid-if-parsed")
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(output.stdout, b"1.9.0\n");
    assert!(output.stderr.is_empty());

    let error = Command::new(binary)
        .arg("--definitely-invalid")
        .output()
        .unwrap();
    assert!(!error.status.success());
    assert!(String::from_utf8_lossy(&error.stderr).starts_with("ronin: "));
}

// [spec:samurai:req:compat.ninja-owned-names/test]
#[test]
fn default_manifest_and_state_files_keep_ninja_names() {
    let directory = test_directory("ninja-names");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("build.ninja"),
        "rule emit\n  command = printf ronin > $out\nbuild output: emit\ndefault output\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_ronin"))
        .current_dir(&directory)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(directory.join("output")).unwrap(),
        "ronin"
    );
    assert!(directory.join(".ninja_log").exists());
    assert!(directory.join(".ninja_deps").exists());
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
// [spec:samurai:req:compat.byte-inputs/test]
#[test]
fn accepts_a_non_utf8_manifest_argument() {
    use std::os::unix::ffi::OsStringExt;

    let directory = test_directory("byte-argument");
    fs::create_dir_all(&directory).unwrap();
    let mut manifest_name = b"build-".to_vec();
    manifest_name.push(0xff);
    manifest_name.extend_from_slice(b".ninja");
    let manifest = directory.join(std::ffi::OsString::from_vec(manifest_name));
    fs::write(
        &manifest,
        "rule emit\n  command = printf exact > $out\nbuild output: emit\ndefault output\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_ronin"))
        .current_dir(&directory)
        .arg("-f")
        .arg(&manifest)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(directory.join("output")).unwrap(),
        "exact"
    );
    fs::remove_dir_all(directory).unwrap();
}
