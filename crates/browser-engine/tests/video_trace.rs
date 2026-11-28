//! `browser_trace {video:true}` against hand's own loopback fixture:
//! a real CDP screencast must land as a playable MJPEG AVI next to the
//! manifest, and cancel must remove exactly what the tool created.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{sleep, JoinHandle};
use std::time::Duration;

use browser_engine::{call, Browser};
use serde_json::{json, Value};

const INDEX: &str = r##"<!doctype html><html><head><meta charset="utf-8"><title>Video fixture</title></head>
<body><h1 id="title">Video fixture</h1><div id="box">0</div>
<script>
let n = 0;
setInterval(() => { n += 1; const b = document.getElementById('box');
  b.textContent = 'frame ' + n; b.style.transform = 'translateX(' + (n % 240) + 'px)'; }, 60);
</script></body></html>"##;

static ONE_BROWSER: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct Fixture {
    base: String,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Fixture {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture");
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");
        let port = listener.local_addr().expect("addr").port();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let handle = std::thread::spawn(move || {
            while !flag.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        std::thread::spawn(move || {
                            let _ = serve(stream);
                        });
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });
        Fixture {
            base: format!("http://127.0.0.1:{port}"),
            stop,
            handle: Some(handle),
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn serve(mut stream: TcpStream) -> std::io::Result<()> {
    // Windows accepted sockets inherit nonblocking mode from the listener.
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request = String::new();
    reader.read_line(&mut request)?;
    let path = request.split_whitespace().nth(1).unwrap_or("/");
    let (status, body) = match path {
        "/" => ("200 OK", INDEX.to_string()),
        "/favicon.ico" => ("204 No Content", String::new()),
        _ => ("404 Not Found", "<h1>missing</h1>".to_string()),
    };
    let response = format!(
        concat!(
            "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {len}\r\n",
            "Cache-Control: no-store\r\nConnection: close\r\n\r\n{body}"
        ),
        status = status, body = body, len = body.len()
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

fn structured(resp: &Value) -> &Value {
    resp.get("structuredContent")
        .unwrap_or_else(|| panic!("no structuredContent: {resp}"))
}

fn is_error(resp: &Value) -> bool {
    resp.get("isError").and_then(|v| v.as_bool()) == Some(true)
}

async fn navigate(browser: &Browser, url: &str) {
    let mut last = Value::Null;
    for attempt in 0..3 {
        let resp = call("navigate", &json!({"url": url}), browser).await;
        if !is_error(&resp) {
            return;
        }
        last = resp;
        let text = last["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .to_string();
        if !text.contains("ERR_ABORTED") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(150 * (attempt + 1))).await;
    }
    panic!("navigate failed: {last}");
}

fn movi_frames(avi: &[u8]) -> usize {
    let pos = avi
        .windows(4)
        .position(|w| w == b"movi")
        .expect("movi list");
    let size = u32::from_le_bytes(avi[pos - 4..pos].try_into().unwrap()) as usize;
    let end = pos + size;
    let mut at = pos + 4;
    let mut frames = 0;
    while at + 8 <= end && &avi[at..at + 4] == b"00dc" {
        let len = u32::from_le_bytes(avi[at + 4..at + 8].try_into().unwrap()) as usize;
        at += 8 + len + len % 2;
        frames += 1;
    }
    frames
}

#[tokio::test]
async fn video_trace_writes_playable_avi_and_cancel_deletes_it() {
    let _serial = ONE_BROWSER.lock().unwrap_or_else(|e| e.into_inner());
    std::env::remove_var("GLADIUS_CDP_URL");
    let fixture = Fixture::start();
    let dir = tempfile::tempdir().expect("temp trace dir");
    let path = dir.path().to_string_lossy().to_string();
    let browser = Browser::new();
    navigate(&browser, &format!("{}/", fixture.base)).await;

    let resp = call("trace", &json!({"dir": path, "video": true}), &browser).await;
    assert!(!is_error(&resp), "trace failed: {resp}");
    let out = structured(&resp);
    assert_eq!(out["ok"], true);

    let avi_path = dir.path().join("trace.avi");
    let avi = std::fs::read(&avi_path).expect("AVI artifact exists");
    assert!(avi.len() > 0, "AVI must not be empty");
    assert_eq!(&avi[0..4], b"RIFF", "AVI header");
    assert_eq!(&avi[8..12], b"AVI ", "AVI form type");
    let frames = movi_frames(&avi);
    assert!(frames >= 1, "AVI must carry at least one screencast frame");
    assert!(
        frames >= 2,
        "the ACK must keep the screencast delivering frames"
    );

    let manifest: Value = serde_json::from_slice(
        &std::fs::read(dir.path().join("manifest.json")).expect("manifest exists"),
    )
    .expect("manifest is JSON");
    assert_eq!(manifest["video"]["container"], "mjpeg-avi");
    assert_eq!(manifest["video"]["frames"], frames);
    assert_eq!(manifest["video"]["bytes"], avi.len());
    assert_eq!(manifest["video"]["durationMs"], 3000, "default window");

    let cancelled = call("trace", &json!({"dir": path, "action": "cancel"}), &browser).await;
    assert!(!is_error(&cancelled), "cancel failed: {cancelled}");
    assert!(!avi_path.exists(), "cancel must delete the AVI");
    assert!(
        !dir.path().join("manifest.json").exists(),
        "cancel must delete the manifest"
    );
}

#[tokio::test]
async fn video_duration_above_cap_is_clamped_and_reported() {
    let _serial = ONE_BROWSER.lock().unwrap_or_else(|e| e.into_inner());
    std::env::remove_var("GLADIUS_CDP_URL");
    let fixture = Fixture::start();
    let dir = tempfile::tempdir().expect("temp trace dir");
    let path = dir.path().to_string_lossy().to_string();
    let browser = Browser::new();
    navigate(&browser, &format!("{}/", fixture.base)).await;

    let resp = call(
        "trace",
        &json!({"dir": path, "video": true, "videoDurationMs": 99_999}),
        &browser,
    )
    .await;
    assert!(!is_error(&resp), "trace failed: {resp}");
    let out = structured(&resp);
    assert!(
        out["note"]
            .as_str()
            .unwrap_or("")
            .contains("clamped down to 15000 ms cap"),
        "{out}"
    );

    let manifest: Value = serde_json::from_slice(
        &std::fs::read(dir.path().join("manifest.json")).expect("manifest exists"),
    )
    .expect("manifest is JSON");
    assert_eq!(manifest["video"]["requestedDurationMs"], 99_999);
    assert_eq!(manifest["video"]["durationMs"], 15_000, "cap is enforced");
    assert_eq!(manifest["video"]["clamped"], true);
    assert!(dir.path().join("trace.avi").exists());

    let cancelled = call("trace", &json!({"dir": path, "action": "cancel"}), &browser).await;
    assert!(!is_error(&cancelled), "cancel failed: {cancelled}");
}

#[tokio::test]
async fn video_avi_is_listed_in_manifest_and_reply() {
    let _serial = ONE_BROWSER.lock().unwrap_or_else(|e| e.into_inner());
    std::env::remove_var("GLADIUS_CDP_URL");
    let fixture = Fixture::start();
    let dir = tempfile::tempdir().expect("temp trace dir");
    let path = dir.path().to_string_lossy().to_string();
    let browser = Browser::new();
    navigate(&browser, &format!("{}/", fixture.base)).await;

    let resp = call(
        "trace",
        &json!({"dir": path, "video": true, "videoDurationMs": 1000}),
        &browser,
    )
    .await;
    assert!(!is_error(&resp), "trace failed: {resp}");
    let out = structured(&resp);
    let avi_len = std::fs::metadata(dir.path().join("trace.avi"))
        .expect("AVI exists")
        .len() as u64;

    let reply_files = out["files"].as_array().expect("reply files");
    let avi_file = reply_files
        .iter()
        .find(|f| f["name"] == "trace.avi")
        .expect("reply files include trace.avi");
    assert_eq!(avi_file["bytes"], avi_len);
    assert_eq!(avi_file["kind"], "video");
    assert!(avi_file["path"]
        .as_str()
        .unwrap_or("")
        .ends_with("trace.avi"));

    let manifest: Value = serde_json::from_slice(
        &std::fs::read(dir.path().join("manifest.json")).expect("manifest exists"),
    )
    .expect("manifest is JSON");
    let manifest_files = manifest["files"].as_array().expect("manifest files");
    assert!(
        manifest_files
            .iter()
            .any(|f| f["name"] == "trace.avi" && f["bytes"] == avi_len),
        "manifest files include the AVI: {manifest_files:?}"
    );
    let listed_total: u64 = manifest_files
        .iter()
        .map(|f| f["bytes"].as_u64().unwrap_or(0))
        .sum();
    // The manifest lists the artifacts it describes, not itself; the reply's
    // byte total also counts manifest.json.
    let manifest_len = std::fs::metadata(dir.path().join("manifest.json"))
        .expect("manifest exists")
        .len();
    assert_eq!(
        out["bytes"],
        listed_total + manifest_len,
        "aggregate bytes include the AVI"
    );
    assert!(out["bytes"].as_u64().unwrap_or(0) >= avi_len);

    let cancelled = call("trace", &json!({"dir": path, "action": "cancel"}), &browser).await;
    assert!(!is_error(&cancelled), "cancel failed: {cancelled}");
}

#[tokio::test]
async fn trace_refuses_to_overwrite_a_preexisting_target() {
    let _serial = ONE_BROWSER.lock().unwrap_or_else(|e| e.into_inner());
    std::env::remove_var("GLADIUS_CDP_URL");
    let browser = Browser::new();

    let dir = tempfile::tempdir().expect("temp trace dir");
    let avi = dir.path().join("trace.avi");
    std::fs::write(&avi, b"caller-owned").expect("seed pre-existing AVI");
    let resp = call(
        "trace",
        &json!({"dir": dir.path().to_string_lossy(), "video": true}),
        &browser,
    )
    .await;
    assert!(
        is_error(&resp),
        "an existing target must be refused: {resp}"
    );
    let err = structured(&resp);
    assert_eq!(err["code"], "invalid_args");
    assert_eq!(err["reason"], "recorded");
    assert!(
        err["path"].as_str().unwrap_or("").ends_with("trace.avi"),
        "{err}"
    );
    assert_eq!(std::fs::read(&avi).expect("seed survives"), b"caller-owned");
    assert!(
        !dir.path().join("manifest.json").exists(),
        "no partial bundle"
    );

    let dir = tempfile::tempdir().expect("temp trace dir");
    let manifest = dir.path().join("manifest.json");
    std::fs::write(&manifest, b"caller-owned").expect("seed pre-existing manifest");
    let resp = call(
        "trace",
        &json!({"dir": dir.path().to_string_lossy()}),
        &browser,
    )
    .await;
    assert!(
        is_error(&resp),
        "an existing manifest must be refused: {resp}"
    );
    assert_eq!(
        std::fs::read(&manifest).expect("seed survives"),
        b"caller-owned"
    );
}

#[ignore = "requires ffprobe on PATH; run with cargo test -- --ignored"]
#[tokio::test]
async fn ffprobe_reads_one_mjpeg_stream() {
    if std::process::Command::new("ffprobe")
        .arg("-version")
        .output()
        .is_err()
    {
        eprintln!("ffprobe not on PATH; skipping");
        return;
    }
    let _serial = ONE_BROWSER.lock().unwrap_or_else(|e| e.into_inner());
    std::env::remove_var("GLADIUS_CDP_URL");
    let fixture = Fixture::start();
    let dir = tempfile::tempdir().expect("temp trace dir");
    let path = dir.path().to_string_lossy().to_string();
    let browser = Browser::new();
    navigate(&browser, &format!("{}/", fixture.base)).await;

    let resp = call(
        "trace",
        &json!({"dir": path, "video": true, "videoDurationMs": 1000}),
        &browser,
    )
    .await;
    assert!(!is_error(&resp), "trace failed: {resp}");
    let avi_path = dir.path().join("trace.avi");
    let out = std::process::Command::new("ffprobe")
        .args(["-v", "error", "-show_streams", "-of", "json"])
        .arg(&avi_path)
        .output()
        .expect("run ffprobe");
    assert!(
        out.status.success(),
        "ffprobe failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let probe: Value = serde_json::from_slice(&out.stdout).expect("ffprobe emits JSON");
    let streams = probe["streams"].as_array().expect("ffprobe streams");
    assert_eq!(streams.len(), 1, "exactly one video stream: {probe}");
    assert_eq!(streams[0]["codec_type"], "video");
    assert_eq!(streams[0]["codec_name"], "mjpeg");

    let cancelled = call("trace", &json!({"dir": path, "action": "cancel"}), &browser).await;
    assert!(!is_error(&cancelled), "cancel failed: {cancelled}");
}
