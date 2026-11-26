//! Regression tests for bounded verified download staging.

use std::fs;
#[cfg(windows)]
use std::fs::OpenOptions;
use std::io::Write as _;
#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt as _;
use std::path::PathBuf;
use std::time::Duration;

use super::*;
use crate::update::release::parse_sha256;

/// Removing streamed digest enforcement would allow corrupted bytes to reach the executable
/// staging path. This oracle uses the published SHA-256 test vector for `abc`.
#[test]
fn streamed_download_rejects_a_digest_mismatch() {
    let expected =
        parse_sha256("sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
            .unwrap();
    let mut written = Vec::new();
    copy_verified(&b"abc"[..], &mut written, 3, &expected).unwrap();
    assert_eq!(written, b"abc");

    let mut corrupted = expected;
    corrupted[0] ^= 0xff;
    let error = copy_verified(&b"abc"[..], Vec::new(), 3, &corrupted).unwrap_err();
    assert!(error.to_string().contains("SHA-256"));
    assert!(copy_verified(&b"abc"[..], Vec::new(), 4, &expected).is_err());
    assert!(copy_verified(&b"abc"[..], Vec::new(), 2, &expected).is_err());
}

/// A failed post-sync verification must remove its part file, while success promotes only the
/// independently rehashed bytes from the still-exclusive open handle.
#[test]
fn part_guard_cleans_failures_and_promotion_keeps_verified_bytes() {
    let root = test_root("part-promotion");
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
    fs::create_dir_all(&root).unwrap();
    let staged = root.join("MoonTerminal.exe");
    let expected =
        parse_sha256("sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
            .unwrap();
    let (path, mut file) = create_unique_part(&staged).unwrap();
    file.write_all(b"bad").unwrap();
    let mut part = PartFile::new(path.clone(), file);
    let part_path = part.path().to_path_buf();
    assert!(promote_verified_part(&part_path, part.file_mut(), &staged, 3, &expected).is_err());
    drop(part);
    assert!(!path.exists());
    assert!(!staged.exists());

    let (path, mut file) = create_unique_part(&staged).unwrap();
    file.write_all(b"abc").unwrap();
    let mut part = PartFile::new(path, file);
    let part_path = part.path().to_path_buf();
    promote_verified_part(&part_path, part.file_mut(), &staged, 3, &expected).unwrap();
    part.disarm();
    assert_eq!(fs::read(&staged).unwrap(), b"abc");
    assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
    fs::remove_dir_all(root).unwrap();
}

/// Reopening the part path for the post-sync hash would fail while this handle deliberately
/// denies shared reads, but hashing the supplied handle can still promote the verified bytes.
#[cfg(windows)]
#[test]
fn post_sync_hash_uses_the_supplied_handle_without_reopening_the_path() {
    const FILE_SHARE_WRITE: u32 = 0x0000_0002;
    const FILE_SHARE_DELETE: u32 = 0x0000_0004;

    let root = test_root("same-handle");
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
    fs::create_dir_all(&root).unwrap();
    let part_path = root.join("MoonTerminal.exe.part");
    let staged = root.join("MoonTerminal.exe");
    let expected =
        parse_sha256("sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
            .unwrap();
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .share_mode(FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .open(&part_path)
        .unwrap();
    file.write_all(b"abc").unwrap();

    assert!(fs::File::open(&part_path).is_err());
    promote_verified_part(&part_path, &mut file, &staged, 3, &expected).unwrap();

    drop(file);
    assert_eq!(fs::read(&staged).unwrap(), b"abc");
    fs::remove_dir_all(root).unwrap();
}

/// A release asset outgrows any whole-call ceiling on an ordinary link: a body still arriving
/// steadily when the release-list ceiling runs out must finish and be promoted.
#[test]
fn asset_download_outlives_the_release_list_ceiling() {
    let root = test_root("slow-body");
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
    fs::create_dir_all(&root).unwrap();
    let staged = root.join("MoonTerminal.exe");
    let server = BodyServer::start(3, &[b"a", b"b", b"c"], Duration::from_millis(700));
    let client =
        GitHubReleaseClient::for_test_with_metadata_timeout(&server.url, Duration::from_secs(1));
    client
        .stage_verified_asset(
            &server.url,
            &staged,
            3,
            &abc_digest(),
            "MoonTerminal-updater",
            1024,
        )
        .unwrap();
    server.finish();
    assert_eq!(fs::read(&staged).unwrap(), b"abc");
    fs::remove_dir_all(root).unwrap();
}

/// The update button shows only the error's outermost text, so a body cut short must name its
/// transport cause there instead of the bare step name.
#[test]
fn a_dropped_body_names_its_cause_in_the_outer_message() {
    let root = test_root("dropped-body");
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
    fs::create_dir_all(&root).unwrap();
    let staged = root.join("MoonTerminal.exe");
    let server = BodyServer::start(6, &[b"abc"], Duration::ZERO);
    let client = GitHubReleaseClient::for_test(&server.url);
    let error = client
        .stage_verified_asset(
            &server.url,
            &staged,
            6,
            &abc_digest(),
            "MoonTerminal-updater",
            1024,
        )
        .unwrap_err();
    server.finish();
    let message = error.to_string();
    let cause = message
        .strip_prefix("read update response: ")
        .unwrap_or_else(|| panic!("unexpected message: {message}"));
    assert!(!cause.trim().is_empty(), "no cause in: {message}");
    assert!(!staged.exists());
    fs::remove_dir_all(root).unwrap();
}

/// Published SHA-256 test vector for `abc`.
fn abc_digest() -> [u8; 32] {
    parse_sha256("sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad").unwrap()
}

/// One-shot loopback server that announces a body length and then writes timed chunks.
struct BodyServer {
    url: String,
    worker: std::thread::JoinHandle<()>,
}

impl BodyServer {
    /// Accept one request, announce `length` bytes, then write each chunk after `pause`.
    fn start(length: usize, chunks: &[&'static [u8]], pause: Duration) -> Self {
        use std::io::Read as _;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let chunks = chunks.to_vec();
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let count = stream.read(&mut buffer).unwrap();
                if count == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..count]);
            }
            let head =
                format!("HTTP/1.1 200 OK\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n");
            stream.write_all(head.as_bytes()).unwrap();
            stream.flush().unwrap();
            for chunk in chunks {
                std::thread::sleep(pause);
                // The client may already have given up; the test asserts on its side.
                if stream
                    .write_all(chunk)
                    .and_then(|()| stream.flush())
                    .is_err()
                {
                    return;
                }
            }
        });
        Self {
            url: format!("http://{address}/asset"),
            worker,
        }
    }

    /// Join the server thread once the client returned.
    fn finish(self) {
        self.worker.join().unwrap();
    }
}

/// Allocate a process-local temporary directory outside every production path constructor.
fn test_root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "moonterminal-update-{label}-{}",
        std::process::id()
    ))
}
