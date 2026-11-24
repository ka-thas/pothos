mod exif;

use axum::{
    extract::{Path as UrlPath, State},
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
    time::UNIX_EPOCH,
};
use tokio::sync::Mutex;

const INDEX: &str = include_str!("../web/index.html");
const AIRDROP_HELPER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/airdrop-helper"));
const SIDECAR: &str = ".pothos.json";
const REJECT_DIR: &str = "_rejected";
const COLORS: [&str; 5] = ["red", "yellow", "green", "blue", "purple"];

#[derive(Default, Serialize, Deserialize)]
struct Sidecar {
    #[serde(default)]
    tags: BTreeMap<String, Vec<String>>,
}

struct App {
    dir: PathBuf,
    side: Mutex<Sidecar>,
    undo: Mutex<Vec<Vec<String>>>,
    helper: Option<PathBuf>,
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

fn is_image(p: &Path) -> bool {
    matches!(
        p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
        Some("jpg" | "jpeg")
    )
}

/// Only plain file names in the folder are allowed (no paths, no dotfiles).
fn safe(app: &App, name: &str) -> Result<PathBuf, (StatusCode, String)> {
    if name.is_empty() || name.starts_with('.') || name.contains(['/', '\\']) {
        return Err(bad("bad name"));
    }
    let p = app.dir.join(name);
    if !p.is_file() || !is_image(&p) {
        return Err((StatusCode::NOT_FOUND, format!("{name} not found")));
    }
    Ok(p)
}

fn version(p: &Path) -> String {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos().to_string())
        .unwrap_or_default()
}

fn save_sidecar(app: &App, s: &Sidecar) -> std::io::Result<()> {
    let tmp = app.dir.join(".pothos.json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(s)?)?;
    std::fs::rename(tmp, app.dir.join(SIDECAR))
}

async fn index() -> Html<&'static str> {
    Html(INDEX)
}

async fn list(State(app): S) -> R {
    let side = app.side.lock().await;
    let mut items: Vec<Item> = std::fs::read_dir(&app.dir)
        .map_err(bad)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file() && is_image(p))
        .filter_map(|p| {
            let name = p.file_name()?.to_str()?.to_string();
            if name.starts_with('.') {
                return None;
            }
            Some(Item {
                tags: side.tags.get(&name).cloned().unwrap_or_default(),
                v: version(&p),
                name,
            })
        })
        .collect();
    items.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    let folder = app.dir.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
    Ok(Json(json!({ "folder": folder, "items": items, "colors": COLORS, "airdrop": app.helper.is_some() }))
        .into_response())
}

async fn image(State(app): S, UrlPath(name): UrlPath<String>) -> R {
    let p = safe(&app, &name)?;
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
    names: Vec<String>,
    cw: bool,
}

async fn rotate(State(app): S, Json(r): Json<RotateReq>) -> R {
    let mut out = BTreeMap::new();
    for name in r.names {
        let p = safe(&app, &name)?;
        let data = tokio::fs::read(&p).await.map_err(bad)?;
        let rotated = exif::rotate(&data, r.cw).map_err(|e| bad(format!("{name}: {e}")))?;
        let tmp = p.with_file_name(format!(".{name}.pothos.tmp"));
        tokio::fs::write(&tmp, rotated).await.map_err(bad)?;
        tokio::fs::rename(&tmp, &p).await.map_err(bad)?;
        out.insert(name, version(&p));
    }
    Ok(Json(out).into_response())
}

#[derive(Deserialize)]
struct TagReq {
    names: Vec<String>,
    /// A color to toggle, or null to clear all tags.
    color: Option<String>,
}

async fn tag(State(app): S, Json(r): Json<TagReq>) -> R {
    let mut side = app.side.lock().await;
    for n in &r.names {
        safe(&app, n)?;
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
    save_sidecar(&app, &side).map_err(bad)?;
    let out: BTreeMap<_, _> = r
        .names
        .iter()
        .map(|n| (n.clone(), side.tags.get(n).cloned().unwrap_or_default()))
        .collect();
    Ok(Json(out).into_response())
}

#[derive(Deserialize)]
struct NamesReq {
    names: Vec<String>,
}

async fn reject(State(app): S, Json(r): Json<NamesReq>) -> R {
    let dest = app.dir.join(REJECT_DIR);
    tokio::fs::create_dir_all(&dest).await.map_err(bad)?;
    let mut moved = vec![];
    for name in r.names {
        let p = safe(&app, &name)?;
        if dest.join(&name).exists() {
            return Err(bad(format!("{name} already in {REJECT_DIR}")));
        }
        tokio::fs::rename(&p, dest.join(&name)).await.map_err(bad)?;
        moved.push(name);
    }
    app.undo.lock().await.push(moved.clone());
    Ok(Json(moved).into_response())
}

async fn undo(State(app): S) -> R {
    let Some(batch) = app.undo.lock().await.pop() else {
        return Ok(Json(Vec::<String>::new()).into_response());
    };
    let src = app.dir.join(REJECT_DIR);
    for name in &batch {
        tokio::fs::rename(src.join(name), app.dir.join(name)).await.map_err(bad)?;
    }
    Ok(Json(batch).into_response())
}

async fn airdrop(State(app): S, Json(r): Json<NamesReq>) -> R {
    let Some(helper) = &app.helper else {
        return Err(bad("AirDrop is only available on macOS"));
    };
    let mut paths = vec![];
    for n in &r.names {
        paths.push(safe(&app, n)?);
    }
    let mut child = tokio::process::Command::new(helper).args(&paths).spawn().map_err(bad)?;
    tokio::spawn(async move {
        let _ = child.wait().await;
    });
    Ok(Json(json!({ "ok": true, "count": paths.len() })).into_response())
}

/// Writes the embedded Swift helper to a temp file so the binary stays self-contained.
fn install_helper() -> Option<PathBuf> {
    if AIRDROP_HELPER.is_empty() {
        return None;
    }
    let p = std::env::temp_dir().join(format!("pothos-airdrop-{}", env!("CARGO_PKG_VERSION")));
    let same = std::fs::read(&p).map(|b| b == AIRDROP_HELPER).unwrap_or(false);
    if !same {
        std::fs::write(&p, AIRDROP_HELPER).ok()?;
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
    let side = std::fs::read(dir.join(SIDECAR))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();

    let app = Arc::new(App {
        dir: dir.clone(),
        side: Mutex::new(side),
        undo: Mutex::new(vec![]),
        helper: install_helper(),
    });

    let router = Router::new()
        .route("/", get(index))
        .route("/api/list", get(list))
        .route("/img/{name}", get(image))
        .route("/api/rotate", post(rotate))
        .route("/api/tag", post(tag))
        .route("/api/reject", post(reject))
        .route("/api/undo", post(undo))
        .route("/api/airdrop", post(airdrop))
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
