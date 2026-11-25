mod exif;
mod publish;

use axum::{
    extract::{Query, State},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::Mutex;

const INDEX: &str = include_str!("../web/index.html");
const AIRDROP_HELPER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/airdrop-helper"));
const PUBLISH_HELPER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/publish-helper"));
const SIDECAR: &str = ".pothos.json";
const REJECT_DIR: &str = "_rejected";
const COLORS: [&str; 5] = ["red", "yellow", "green", "blue", "purple"];

#[derive(Default, Serialize, Deserialize)]
struct Sidecar {
    #[serde(default)]
    tags: BTreeMap<String, Vec<String>>,
}

struct App {
    /// The folder pothos was started on. Subfolders are browsed relative to it and never above it.
    dir: PathBuf,
    /// Serializes sidecar read-modify-write. Each folder has its own `.pothos.json`.
    side: Mutex<()>,
    /// Reject batches: (folder relative to root, file names).
    undo: Mutex<Vec<(String, Vec<String>)>>,
    helper: Option<PathBuf>,
    publish_helper: Option<PathBuf>,
    /// Held while publishing, so two publishes never run at once.
    publishing: Mutex<()>,
}
type S = State<Arc<App>>;

#[derive(Serialize)]
struct Item {
    name: String,
    v: String,
    tags: Vec<String>,
}

type R = Result<Response, (StatusCode, String)>;
fn bad(e: impl ToString) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, e.to_string())
}
fn not_found(what: &str) -> (StatusCode, String) {
    (StatusCode::NOT_FOUND, format!("{what} not found"))
}

fn is_image(p: &Path) -> bool {
    matches!(
        p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
        Some("jpg" | "jpeg")
    )
}

/// Resolves a folder given relative to the root, like `day1/morning`.
/// Only plain names (no dotfiles, no `..`, not `_rejected`), and it must stay inside the root.
fn folder(app: &App, dir: &str) -> Result<PathBuf, (StatusCode, String)> {
    let mut p = app.dir.clone();
    for c in dir.split('/').filter(|c| !c.is_empty()) {
        if c.starts_with('.') || c.contains('\\') || c == REJECT_DIR {
            return Err(bad("bad folder"));
        }
        p.push(c);
    }
    let p = p.canonicalize().map_err(|_| not_found(dir))?;
    if !p.starts_with(&app.dir) || !p.is_dir() {
        return Err(not_found(dir));
    }
    Ok(p)
}

/// Only plain file names in the folder are allowed (no paths, no dotfiles).
fn safe(dir: &Path, name: &str) -> Result<PathBuf, (StatusCode, String)> {
    if name.is_empty() || name.starts_with('.') || name.contains(['/', '\\']) {
        return Err(bad("bad name"));
    }
    let p = dir.join(name);
    if !p.is_file() || !is_image(&p) {
        return Err(not_found(name));
    }
    Ok(p)
}

/// Cache-busting version for image URLs. Uses the change time, not the modified time:
/// rotation keeps the modified time (the only capture time a camera without EXIF leaves),
/// but every rewrite or rename still bumps the change time.
fn version(p: &Path) -> String {
    let Ok(m) = std::fs::metadata(p) else { return String::new() };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        format!("{}{:09}", m.ctime(), m.ctime_nsec())
    }
    #[cfg(not(unix))]
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos().to_string())
        .unwrap_or_default()
}

/// Visible entry names in a folder matching `keep`, sorted case-insensitively.
fn entries(dir: &Path, keep: impl Fn(&Path, &str) -> bool) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_string();
            (!name.starts_with('.') && keep(&e.path(), &name)).then_some(name)
        })
        .collect();
    v.sort_by_key(|n| n.to_lowercase());
    v
}
fn photos(dir: &Path) -> Vec<String> {
    entries(dir, |p, _| p.is_file() && is_image(p))
}
fn subfolders(dir: &Path) -> Vec<String> {
    entries(dir, |p, n| p.is_dir() && n != REJECT_DIR)
}

fn load_sidecar(dir: &Path) -> Sidecar {
    std::fs::read(dir.join(SIDECAR))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_sidecar(dir: &Path, s: &Sidecar) -> std::io::Result<()> {
    let tmp = dir.join(".pothos.json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(s)?)?;
    std::fs::rename(tmp, dir.join(SIDECAR))
}

async fn index() -> Html<&'static str> {
    Html(INDEX)
}

#[derive(Deserialize)]
struct DirQuery {
    #[serde(default)]
    dir: String,
}

async fn list(State(app): S, Query(q): Query<DirQuery>) -> R {
    let d = folder(&app, &q.dir)?;
    let side = {
        let _guard = app.side.lock().await;
        load_sidecar(&d)
    };
    let items: Vec<Item> = photos(&d)
        .into_iter()
        .map(|name| Item {
            tags: side.tags.get(&name).cloned().unwrap_or_default(),
            v: version(&d.join(&name)),
            name,
        })
        .collect();
    let folders: Vec<_> = subfolders(&d)
        .into_iter()
        .map(|name| {
            let p = d.join(&name);
            let ph = photos(&p);
            let cover = ph.first().map(|c| json!({ "name": c, "v": version(&p.join(c)) }));
            json!({ "name": name, "photos": ph.len(), "folders": subfolders(&p).len(), "cover": cover })
        })
        .collect();
    let root = app.dir.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
    Ok(Json(json!({
        "root": root, "items": items, "folders": folders, "colors": COLORS, "airdrop": app.helper.is_some(),
        "publish": app.publish_helper.is_some() && publish::config_path().exists()
    }))
    .into_response())
}

#[derive(Deserialize)]
struct ImageQuery {
    #[serde(default)]
    dir: String,
    name: String,
}

async fn image(State(app): S, Query(q): Query<ImageQuery>) -> R {
    let p = safe(&folder(&app, &q.dir)?, &q.name)?;
    let bytes = tokio::fs::read(&p).await.map_err(bad)?;
    Ok((
        [
            (header::CONTENT_TYPE, "image/jpeg"),
            // URLs carry ?v=<mtime>, so a changed file gets a new URL
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
        ],
        bytes,
    )
        .into_response())
}

#[derive(Deserialize)]
struct RotateReq {
    #[serde(default)]
    dir: String,
    names: Vec<String>,
    cw: bool,
}

async fn rotate(State(app): S, Json(r): Json<RotateReq>) -> R {
    let d = folder(&app, &r.dir)?;
    let mut out = BTreeMap::new();
    for name in r.names {
        let p = safe(&d, &name)?;
        let data = tokio::fs::read(&p).await.map_err(bad)?;
        let taken = std::fs::metadata(&p).and_then(|m| m.modified()).map_err(bad)?;
        let rotated = exif::rotate(&data, r.cw).map_err(|e| bad(format!("{name}: {e}")))?;
        let tmp = p.with_file_name(format!(".{name}.pothos.tmp"));
        tokio::fs::write(&tmp, rotated).await.map_err(bad)?;
        // Keep the original modified time: it is the capture time for cameras without EXIF dates.
        std::fs::File::options().write(true).open(&tmp).and_then(|f| f.set_modified(taken)).map_err(bad)?;
        tokio::fs::rename(&tmp, &p).await.map_err(bad)?;
        out.insert(name, version(&p));
    }
    Ok(Json(out).into_response())
}

#[derive(Deserialize)]
struct TagReq {
    #[serde(default)]
    dir: String,
    names: Vec<String>,
    /// A color to toggle, or null to clear all tags.
    color: Option<String>,
}

async fn tag(State(app): S, Json(r): Json<TagReq>) -> R {
    let d = folder(&app, &r.dir)?;
    let _guard = app.side.lock().await;
    let mut side = load_sidecar(&d);
    for n in &r.names {
        safe(&d, n)?;
    }
    match &r.color {
        None => {
            for n in &r.names {
                side.tags.remove(n);
            }
        }
        Some(c) => {
            if !COLORS.contains(&c.as_str()) {
                return Err(bad("unknown color"));
            }
            // Batch toggle: if every item already has it, remove; otherwise add to all.
            let all_have = r.names.iter().all(|n| side.tags.get(n).is_some_and(|t| t.contains(c)));
            for n in &r.names {
                let t = side.tags.entry(n.clone()).or_default();
                t.retain(|x| x != c);
                if !all_have {
                    t.push(c.clone());
                    t.sort_by_key(|x| COLORS.iter().position(|k| k == x));
                }
                if t.is_empty() {
                    side.tags.remove(n);
                }
            }
        }
    }
    save_sidecar(&d, &side).map_err(bad)?;
    let out: BTreeMap<_, _> = r
        .names
        .iter()
        .map(|n| (n.clone(), side.tags.get(n).cloned().unwrap_or_default()))
        .collect();
    Ok(Json(out).into_response())
}

#[derive(Deserialize)]
struct NamesReq {
    #[serde(default)]
    dir: String,
    names: Vec<String>,
}

async fn reject(State(app): S, Json(r): Json<NamesReq>) -> R {
    let d = folder(&app, &r.dir)?;
    let dest = d.join(REJECT_DIR);
    tokio::fs::create_dir_all(&dest).await.map_err(bad)?;
    let mut moved = vec![];
    for name in r.names {
        let p = safe(&d, &name)?;
        if dest.join(&name).exists() {
            return Err(bad(format!("{name} already in {REJECT_DIR}")));
        }
        tokio::fs::rename(&p, dest.join(&name)).await.map_err(bad)?;
        moved.push(name);
    }
    app.undo.lock().await.push((r.dir, moved.clone()));
    Ok(Json(moved).into_response())
}

async fn undo(State(app): S) -> R {
    let Some((dir, batch)) = app.undo.lock().await.pop() else {
        return Ok(Json(json!({ "dir": "", "names": [] })).into_response());
    };
    let d = folder(&app, &dir)?;
    let src = d.join(REJECT_DIR);
    for name in &batch {
        tokio::fs::rename(src.join(name), d.join(name)).await.map_err(bad)?;
    }
    Ok(Json(json!({ "dir": dir, "names": batch })).into_response())
}

async fn airdrop(State(app): S, Json(r): Json<NamesReq>) -> R {
    let Some(helper) = &app.helper else {
        return Err(bad("AirDrop is only available on macOS"));
    };
    let d = folder(&app, &r.dir)?;
    let mut paths = vec![];
    for n in &r.names {
        paths.push(safe(&d, n)?);
    }
    let mut child = tokio::process::Command::new(helper).args(&paths).spawn().map_err(bad)?;
    tokio::spawn(async move {
        let _ = child.wait().await;
    });
    Ok(Json(json!({ "ok": true, "count": paths.len() })).into_response())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PublishReq {
    #[serde(default)]
    dry_run: bool,
}

/// Publishes every green photo under the root (all subfolders) to the R2 bucket.
async fn publish(State(app): S, Json(r): Json<PublishReq>) -> R {
    let Some(helper) = &app.publish_helper else {
        return Err(bad("Publishing is only available on macOS"));
    };
    let Ok(_guard) = app.publishing.try_lock() else {
        return Err(bad("Already publishing"));
    };
    let out = publish::run(&app.dir, helper, r.dry_run).await.map_err(bad)?;
    Ok(Json(out).into_response())
}

/// Writes an embedded Swift helper to a temp file so the binary stays self-contained.
fn install_helper(name: &str, bytes: &[u8]) -> Option<PathBuf> {
    if bytes.is_empty() {
        return None;
    }
    let p = std::env::temp_dir().join(format!("pothos-{name}-{}", env!("CARGO_PKG_VERSION")));
    let same = std::fs::read(&p).map(|b| b == bytes).unwrap_or(false);
    if !same {
        std::fs::write(&p, bytes).ok()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).ok()?;
        }
    }
    Some(p)
}

#[tokio::main]
async fn main() {
    let mut dir = None;
    let mut port: u16 = 8787;
    let mut open = true;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "-p" | "--port" => port = args.next().and_then(|p| p.parse().ok()).expect("--port needs a number"),
            "--no-open" => open = false,
            "-h" | "--help" => {
                println!("pothos [FOLDER] [--port 8787] [--no-open]");
                return;
            }
            _ => dir = Some(PathBuf::from(a)),
        }
    }
    let dir = dir
        .unwrap_or_else(|| std::env::current_dir().unwrap())
        .canonicalize()
        .expect("folder not found");
    let app = Arc::new(App {
        dir: dir.clone(),
        side: Mutex::new(()),
        undo: Mutex::new(vec![]),
        helper: install_helper("airdrop", AIRDROP_HELPER),
        publish_helper: install_helper("publish", PUBLISH_HELPER),
        publishing: Mutex::new(()),
    });

    let router = Router::new()
        .route("/", get(index))
        .route("/api/list", get(list))
        .route("/img", get(image))
        .route("/api/rotate", post(rotate))
        .route("/api/tag", post(tag))
        .route("/api/reject", post(reject))
        .route("/api/undo", post(undo))
        .route("/api/airdrop", post(airdrop))
        .route("/api/publish", post(publish))
        .with_state(app);

    // Localhost only: nothing is exposed to the network.
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .unwrap_or_else(|e| panic!("cannot bind port {port}: {e}"));
    let url = format!("http://127.0.0.1:{port}");
    println!("pothos: {}  ->  {url}", dir.display());
    if open {
        let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
        let _ = std::process::Command::new(opener).arg(&url).spawn();
    }
    axum::serve(listener, router).await.unwrap();
}
