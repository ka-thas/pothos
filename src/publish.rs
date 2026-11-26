//! Publishes green photos to a Cloudflare R2 bucket for the kathas.no photo page.
//!
//! Bucket layout: `manifest.json` at the root, full images under `photos/`, thumbnails under `thumbs/`.
//! Image names include a hash of the source file, so a name is never reused for different pixels
//! (images are cached forever). Images go up first, the manifest last, and files the new manifest
//! no longer uses are deleted after that, so the site never points at a missing file.
//!
//! The Swift helper (publish.swift) does the image work and the signed HTTP requests.
use crate::{load_sidecar, photos, subfolders, REJECT_DIR};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// Photos with this tag are published.
pub const TAG: &str = "green";
const THUMB_PX: &str = "540";
const IMAGE_CACHE: &str = "public, max-age=31536000, immutable";
const MANIFEST_CACHE: &str = "public, max-age=60";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    endpoint: String,
    bucket: String,
    access_key_id: String,
    secret_access_key: String,
    #[serde(default)]
    trips: Vec<Trip>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Trip {
    id: String,
    name: String,
    start: String,
    end: String,
}

pub fn config_path() -> PathBuf {
    if let Some(p) = std::env::var_os("POTHOS_PUBLISH_CONFIG") {
        return p.into();
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join(".config/pothos/publish.json")
}

fn load_config() -> Result<Config, String> {
    let p = config_path();
    let bytes = std::fs::read(&p)
        .map_err(|_| format!("Publishing is not set up: {} not found. See the README.", p.display()))?;
    let c: Config = serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", p.display()))?;
    let local = c.endpoint.starts_with("http://127.0.0.1") || c.endpoint.starts_with("http://localhost");
    if !c.endpoint.starts_with("https://") && !local {
        return Err("endpoint must look like https://<account id>.r2.cloudflarestorage.com".into());
    }
    let mut ids = BTreeSet::new();
    for t in &c.trips {
        if days(&t.start).is_none() || days(&t.end).is_none() || t.start > t.end {
            return Err(format!("trip {}: start and end must be YYYY-MM-DD, with start first", t.id));
        }
        if !ids.insert(&t.id) {
            return Err(format!("trip id {} is used twice", t.id));
        }
    }
    Ok(c)
}

/// Days since 1970-01-01 for a strict YYYY-MM-DD date (Howard Hinnant's days_from_civil).
fn days(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 10 || !s.is_ascii() || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let (y, m, d): (i64, i64, i64) = (s[..4].parse().ok()?, s[5..7].parse().ok()?, s[8..].parse().ok()?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    Some(era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719468)
}

/// UTC timestamp like 2026-11-25T05:00:00Z.
fn iso_utc(secs: i64) -> String {
    let z = secs.div_euclid(86400) + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    let r = secs.rem_euclid(86400);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", r / 3600, r % 3600 / 60, r % 60)
}

/// The trip whose date range holds the local date of `taken_at` (inclusive). The shortest range
/// wins, so a side trip beats a long stay that contains it.
fn trip_for<'a>(taken_at: &str, trips: &'a [Trip]) -> Option<&'a Trip> {
    let date = taken_at.get(..10)?;
    trips
        .iter()
        .filter(|t| t.start.as_str() <= date && date <= t.end.as_str())
        .min_by_key(|t| days(&t.end).unwrap_or(0) - days(&t.start).unwrap_or(0))
}

/// Stable id from the capture time and file name, like 20261121-214704-pict0013.
fn make_id(taken_at: &str, name: &str) -> String {
    let digits: String = taken_at.chars().take(19).filter(|c| c.is_ascii_digit()).collect();
    let stem = Path::new(name).file_stem().and_then(|s| s.to_str()).unwrap_or(name);
    let mut slug = String::new();
    for c in stem.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    let split = digits.len().min(8);
    format!("{}-{}-{}", &digits[..split], &digits[split..], if slug.is_empty() { "photo" } else { slug })
}

/// FNV-1a over the source file. Only names files, so it needs to differ when pixels or
/// rotation change, not to resist attacks.
fn content_hash(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")[..12].to_string()
}

struct Local {
    path: PathBuf,
    id: String,
    taken_at: String,
    hash: String,
    green: bool,
}

/// The file name part of an id and the content hash, like "pict0013-755dc370cef4". It stays the
/// same when only a photo's time changes, so a re-timed photo replaces its old entry.
fn same_photo_key(id: &str, hash: &str) -> Option<String> {
    let name = id.splitn(3, '-').nth(2)?;
    Some(format!("{name}-{hash}"))
}

/// The key of a manifest entry, with the hash taken from its file name.
fn entry_key(p: &Map<String, Value>) -> Option<String> {
    let stem = Path::new(str_of(p, "file")?).file_stem()?.to_str()?;
    same_photo_key(str_of(p, "id")?, stem.rsplit_once('-')?.1)
}

#[derive(Default)]
struct Plan {
    photos: Vec<Map<String, Value>>,
    /// (id, source) for new or changed photos. Their width and height are filled in after rendering.
    uploads: Vec<(String, PathBuf)>,
    /// Keys the old manifest used and the new one does not.
    orphans: Vec<String>,
    new: usize,
    changed: usize,
    unchanged: usize,
    removed: usize,
    kept_other: usize,
    warnings: Vec<String>,
}

fn str_of<'a>(p: &'a Map<String, Value>, k: &str) -> Option<&'a str> {
    p.get(k).and_then(Value::as_str)
}

/// Merges the photos found here into the bucket's current manifest.
/// Photos published from other folders stay. Photos here that are not green are removed.
fn plan(existing: &Value, locals: &[Local], trips: &[Trip]) -> Result<Plan, String> {
    let old: Vec<Map<String, Value>> = match existing.get("photos") {
        None | Some(Value::Null) => vec![],
        Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_object().cloned()).collect(),
        Some(_) => return Err("manifest.json in the bucket has a photos field that is not a list".into()),
    };
    let old_by_id: HashMap<&str, &Map<String, Value>> =
        old.iter().filter_map(|p| Some((str_of(p, "id")?, p))).collect();
    let old_by_key: HashMap<String, &Map<String, Value>> =
        old.iter().filter_map(|p| Some((entry_key(p)?, p))).collect();
    let here: BTreeSet<&str> = locals.iter().map(|l| l.id.as_str()).collect();
    let here_keys: BTreeSet<String> = locals.iter().filter_map(|l| same_photo_key(&l.id, &l.hash)).collect();
    // An old entry is from this folder if its id or its name and contents match a photo here.
    let is_here = |p: &Map<String, Value>| {
        str_of(p, "id").is_some_and(|id| here.contains(id)) || entry_key(p).is_some_and(|k| here_keys.contains(&k))
    };
    let mut out = Plan::default();

    for p in &old {
        if !is_here(p) {
            out.photos.push(p.clone());
            out.kept_other += 1;
        }
    }
    let mut published = BTreeSet::new();
    let mut published_keys = BTreeSet::new();
    for l in locals {
        if !l.green {
            continue;
        }
        let hash = &l.hash;
        if !published.insert(l.id.as_str()) {
            out.warnings.push(format!("skipped {}: same time and name as another green photo", l.path.display()));
            continue;
        }
        let (file, thumb) = (format!("photos/{}-{hash}.jpg", l.id), format!("thumbs/{}-{hash}.jpg", l.id));
        let key = same_photo_key(&l.id, hash);
        let prev = old_by_id.get(l.id.as_str()).or_else(|| key.as_ref().and_then(|k| old_by_key.get(k)));
        published_keys.extend(key);
        let mut e = Map::new();
        e.insert("id".into(), json!(l.id));
        e.insert("file".into(), json!(file));
        e.insert("thumb".into(), json!(thumb));
        e.insert("takenAt".into(), json!(l.taken_at));
        if let Some(c) = prev.and_then(|p| p.get("caption")) {
            e.insert("caption".into(), c.clone()); // captions are kept across publishes
        }
        let same = prev.is_some_and(|p| {
            str_of(p, "file") == Some(file.as_str())
                && str_of(p, "thumb") == Some(thumb.as_str())
                && p.get("width").is_some_and(Value::is_u64)
                && p.get("height").is_some_and(Value::is_u64)
        });
        if let (true, Some(p)) = (same, prev) {
            e.insert("width".into(), p["width"].clone());
            e.insert("height".into(), p["height"].clone());
            out.unchanged += 1;
        } else {
            if prev.is_some() { out.changed += 1 } else { out.new += 1 }
            out.uploads.push((l.id.clone(), l.path.clone()));
        }
        out.photos.push(e);
    }
    out.removed = old
        .iter()
        .filter(|p| is_here(p))
        .filter(|p| !str_of(p, "id").is_some_and(|id| published.contains(id)))
        .filter(|p| !entry_key(p).is_some_and(|k| published_keys.contains(&k)))
        .count();

    for p in &mut out.photos {
        match str_of(p, "takenAt").and_then(|t| trip_for(t, trips)) {
            Some(t) => p.insert("trip".into(), json!(t.id)),
            None => p.remove("trip"),
        };
    }
    out.photos.sort_by(|a, b| {
        (str_of(b, "takenAt"), str_of(b, "id")).cmp(&(str_of(a, "takenAt"), str_of(a, "id")))
    });

    let keys = |ps: &[Map<String, Value>]| -> BTreeSet<String> {
        ps.iter()
            .flat_map(|p| ["file", "thumb"].map(|k| str_of(p, k).map(String::from)))
            .flatten()
            .collect()
    };
    let new_keys = keys(&out.photos);
    out.orphans = keys(&old).into_iter().filter(|k| !new_keys.contains(k)).collect();
    Ok(out)
}

/// Every photo under `root`, with whether it is green. Rejected photos count as not green,
/// so rejecting a published photo takes it off the site.
fn walk(root: &Path) -> Vec<(PathBuf, bool)> {
    fn go(dir: &Path, root: &Path, seen: &mut BTreeSet<PathBuf>, out: &mut Vec<(PathBuf, bool)>) {
        let Ok(real) = dir.canonicalize() else { return };
        if !real.starts_with(root) || !seen.insert(real) {
            return; // outside the root, or a symlink loop
        }
        let side = load_sidecar(dir);
        for name in photos(dir) {
            let green = side.tags.get(&name).is_some_and(|t| t.iter().any(|c| c == TAG));
            out.push((dir.join(name), green));
        }
        let rejected = dir.join(REJECT_DIR);
        for name in photos(&rejected) {
            out.push((rejected.join(name), false));
        }
        for sub in subfolders(dir) {
            go(&dir.join(sub), root, seen, out);
        }
    }
    let mut out = vec![];
    go(root, root, &mut BTreeSet::new(), &mut out);
    out
}

struct Helper<'a> {
    path: &'a Path,
    cfg: &'a Config,
}

impl Helper<'_> {
    /// Runs the helper. Exit code 3 (missing key) is returned as Ok(None).
    async fn run(&self, args: &[&std::ffi::OsStr]) -> Result<Option<Vec<u8>>, String> {
        let out = tokio::process::Command::new(self.path)
            .args(args)
            .env("R2_ENDPOINT", &self.cfg.endpoint)
            .env("R2_BUCKET", &self.cfg.bucket)
            .env("R2_ACCESS_KEY_ID", &self.cfg.access_key_id)
            .env("R2_SECRET_ACCESS_KEY", &self.cfg.secret_access_key)
            .output()
            .await
            .map_err(|e| e.to_string())?;
        match out.status.code() {
            Some(0) => Ok(Some(out.stdout)),
            Some(3) => Ok(None),
            _ => Err(String::from_utf8_lossy(&out.stderr).trim().to_string()),
        }
    }

    async fn put(&self, key: &str, file: &Path, content_type: &str, cache: &str) -> Result<(), String> {
        self.run(&["put".as_ref(), key.as_ref(), file.as_os_str(), content_type.as_ref(), cache.as_ref()])
            .await
            .map(|_| ())
    }
}

/// Removes the temp folder when publishing ends, even on errors.
struct TempDir(PathBuf);
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// With `dry_run`, only reads the bucket and reports what a publish would change.
pub async fn run(root: &Path, helper: &Path, dry_run: bool) -> Result<Value, String> {
    let cfg = load_config()?;
    let h = Helper { path: helper, cfg: &cfg };

    let found = walk(root);
    let mut taken = Vec::with_capacity(found.len());
    for chunk in found.chunks(200) {
        let mut args: Vec<&std::ffi::OsStr> = vec!["info".as_ref()];
        args.extend(chunk.iter().map(|(p, _)| p.as_os_str()));
        let out = h.run(&args).await?.unwrap_or_default();
        let v: Vec<Value> = serde_json::from_slice(&out).map_err(|e| format!("helper info: {e}"))?;
        taken.extend(v.into_iter().map(|x| x["takenAt"].as_str().unwrap_or_default().to_string()));
    }
    if taken.len() != found.len() {
        return Err("helper info returned the wrong number of dates".into());
    }
    let mut locals = vec![];
    for ((path, green), taken_at) in found.into_iter().zip(taken) {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        // Every photo is hashed (they are small), so a re-timed photo is recognised even if not green.
        let hash = content_hash(&std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?);
        locals.push(Local { id: make_id(&taken_at, name), taken_at, hash, green, path });
    }

    let tmp = TempDir(std::env::temp_dir().join(format!("pothos-publish-{}", std::process::id())));
    std::fs::create_dir_all(&tmp.0).map_err(|e| e.to_string())?;
    let old_path = tmp.0.join("manifest-old.json");
    let existing = match h.run(&["get".as_ref(), "manifest.json".as_ref(), old_path.as_os_str()]).await? {
        Some(_) => serde_json::from_slice(&std::fs::read(&old_path).map_err(|e| e.to_string())?)
            .map_err(|e| format!("manifest.json in the bucket is not valid JSON ({e}), so it was left alone"))?,
        None => Value::Null,
    };
    let mut p = plan(&existing, &locals, &cfg.trips)?;
    let summary = |p: &Plan| {
        json!({
            "bucket": cfg.bucket, "dryRun": dry_run,
            "green": p.new + p.changed + p.unchanged, "new": p.new, "changed": p.changed,
            "unchanged": p.unchanged, "removed": p.removed, "keptOther": p.kept_other,
            "deleted": if dry_run { 0 } else { p.orphans.len() }, "warnings": p.warnings,
        })
    };
    if dry_run {
        return Ok(summary(&p));
    }

    // 1. Images. 2. The manifest. 3. Files nothing points at anymore.
    for (i, (id, src)) in p.uploads.iter().enumerate() {
        let (full, thumb) = (tmp.0.join(format!("{i}-full.jpg")), tmp.0.join(format!("{i}-thumb.jpg")));
        let out = h
            .run(&["render".as_ref(), src.as_os_str(), full.as_os_str(), thumb.as_os_str(), THUMB_PX.as_ref()])
            .await?
            .unwrap_or_default();
        let size: Value = serde_json::from_slice(&out).map_err(|e| format!("helper render: {e}"))?;
        let e = p.photos.iter_mut().find(|e| str_of(e, "id") == Some(id.as_str())).expect("planned photo");
        h.put(str_of(e, "file").unwrap_or_default(), &full, "image/jpeg", IMAGE_CACHE).await?;
        h.put(str_of(e, "thumb").unwrap_or_default(), &thumb, "image/jpeg", IMAGE_CACHE).await?;
        e.insert("width".into(), size["width"].clone());
        e.insert("height".into(), size["height"].clone());
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let manifest = json!({ "updated": iso_utc(now), "trips": cfg.trips, "photos": p.photos });
    let new_path = tmp.0.join("manifest.json");
    std::fs::write(&new_path, serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    h.put("manifest.json", &new_path, "application/json", MANIFEST_CACHE).await?;
    for key in &p.orphans {
        if let Err(e) = h.run(&["delete".as_ref(), key.as_ref()]).await {
            p.warnings.push(format!("could not delete {key}: {e}"));
        }
    }
    Ok(summary(&p))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trip(id: &str, start: &str, end: &str) -> Trip {
        Trip { id: id.into(), name: id.into(), start: start.into(), end: end.into() }
    }

    #[test]
    fn dates() {
        assert_eq!(days("1970-01-01"), Some(0));
        assert_eq!(days("2026-11-25"), Some(20782));
        assert_eq!(days("2026-13-01"), None);
        assert_eq!(days("2026-1-01"), None);
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_utc(20782 * 86400 + 5 * 3600 + 61), "2026-11-25T05:01:01Z");
        assert_eq!(iso_utc(951782400), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn shortest_trip_wins_and_ends_are_inclusive() {
        let trips = [trip("nagoya", "2026-09-01", "2027-05-31"), trip("chengdu", "2027-03-03", "2027-03-10")];
        let id = |t: &str| trip_for(t, &trips).map(|t| t.id.clone());
        assert_eq!(id("2027-03-03T08:00:00+09:00").as_deref(), Some("chengdu"));
        assert_eq!(id("2027-03-10T23:59:00+09:00").as_deref(), Some("chengdu"));
        assert_eq!(id("2027-03-11T00:00:00+09:00").as_deref(), Some("nagoya"));
        assert_eq!(id("2026-09-01T00:00:00+09:00").as_deref(), Some("nagoya"));
        assert_eq!(id("2026-08-31T23:59:00+09:00"), None);
    }

    #[test]
    fn ids() {
        assert_eq!(make_id("2026-11-21T21:47:04+09:00", "PICT0013.jpg"), "20261121-214704-pict0013");
        assert_eq!(make_id("2026-11-21T21:47:04+09:00", "My Photo (2).JPG"), "20261121-214704-my-photo-2");
        assert_eq!(make_id("2026-11-21T21:47:04+09:00", "__.jpg"), "20261121-214704-photo");
    }

    fn local(id: &str, green: Option<&str>) -> Local {
        Local {
            path: PathBuf::from(format!("/x/{id}.jpg")),
            id: id.into(),
            taken_at: format!("2026-11-{}T12:00:00+09:00", &id[..2]),
            hash: green.unwrap_or("not-green").into(),
            green: green.is_some(),
        }
    }

    #[test]
    fn merge() {
        let existing = json!({ "photos": [
            { "id": "20-same", "file": "photos/20-same-aaa.jpg", "thumb": "thumbs/20-same-aaa.jpg",
              "takenAt": "2026-11-20T12:00:00+09:00", "width": 1440, "height": 1080, "caption": { "en": "hi" } },
            { "id": "21-edit", "file": "photos/21-edit-old.jpg", "thumb": "thumbs/21-edit-old.jpg",
              "takenAt": "2026-11-21T12:00:00+09:00", "width": 1440, "height": 1080 },
            { "id": "22-gone", "file": "photos/22-gone-ccc.jpg", "thumb": "thumbs/22-gone-ccc.jpg",
              "takenAt": "2026-11-22T12:00:00+09:00", "width": 1440, "height": 1080 },
            { "id": "01-other", "file": "photos/01-other-ddd.jpg", "thumb": "thumbs/01-other-ddd.jpg",
              "takenAt": "2025-01-01T12:00:00+09:00", "width": 10, "height": 10, "trip": "old" },
        ]});
        let locals = [
            local("20-same", Some("aaa")),
            local("21-edit", Some("new")),
            local("22-gone", None),
            local("23-fresh", Some("eee")),
        ];
        let trips = [trip("nagoya", "2026-09-01", "2027-05-31")];
        let p = plan(&existing, &locals, &trips).unwrap();
        assert_eq!((p.new, p.changed, p.unchanged, p.removed, p.kept_other), (1, 1, 1, 1, 1));
        let ids: Vec<_> = p.photos.iter().map(|e| str_of(e, "id").unwrap()).collect();
        assert_eq!(ids, ["23-fresh", "21-edit", "20-same", "01-other"]); // newest first
        let uploads: Vec<_> = p.uploads.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(uploads, ["21-edit", "23-fresh"]);
        assert_eq!(p.photos[2]["caption"]["en"], "hi");
        assert_eq!(p.photos[2]["trip"], "nagoya");
        assert!(p.photos[3].get("trip").is_none()); // no configured trip matches 2025 anymore
        assert_eq!(
            p.orphans,
            ["photos/21-edit-old.jpg", "photos/22-gone-ccc.jpg", "thumbs/21-edit-old.jpg", "thumbs/22-gone-ccc.jpg"]
        );
    }

    #[test]
    fn retimed_photo_replaces_its_old_entry() {
        let existing = json!({ "photos": [
            { "id": "21-120000-pict0013", "file": "photos/21-120000-pict0013-aaa.jpg", "thumb": "thumbs/21-120000-pict0013-aaa.jpg",
              "takenAt": "2026-11-21T12:00:00+09:00", "width": 4, "height": 3, "caption": { "en": "hi" } },
            { "id": "22-120000-pict0014", "file": "photos/22-120000-pict0014-bbb.jpg", "thumb": "thumbs/22-120000-pict0014-bbb.jpg",
              "takenAt": "2026-11-22T12:00:00+09:00", "width": 4, "height": 3 },
            // Same name, other contents: another camera roll, so it stays.
            { "id": "01-120000-pict0013", "file": "photos/01-120000-pict0013-zzz.jpg", "thumb": "thumbs/01-120000-pict0013-zzz.jpg",
              "takenAt": "2026-11-01T12:00:00+09:00", "width": 4, "height": 3 },
        ]});
        // pict0013 got a new time and is still green; pict0014 got a new time and is no longer green.
        let mut retimed = local("23-120000-pict0013", Some("aaa"));
        retimed.taken_at = "2026-11-23T12:00:00+09:00".into();
        let mut ungreen = local("24-120000-pict0014", None);
        ungreen.hash = "bbb".into();
        let p = plan(&existing, &[retimed, ungreen], &[]).unwrap();
        let ids: Vec<_> = p.photos.iter().map(|e| str_of(e, "id").unwrap()).collect();
        assert_eq!(ids, ["23-120000-pict0013", "01-120000-pict0013"]);
        assert_eq!((p.new, p.changed, p.removed, p.kept_other), (0, 1, 1, 1));
        assert_eq!(p.photos[0]["caption"]["en"], "hi");
        assert_eq!(
            p.orphans,
            ["photos/21-120000-pict0013-aaa.jpg", "photos/22-120000-pict0014-bbb.jpg", "thumbs/21-120000-pict0013-aaa.jpg", "thumbs/22-120000-pict0014-bbb.jpg"]
        );
    }

    #[test]
    fn empty_bucket() {
        let p = plan(&Value::Null, &[local("20-a", Some("h"))], &[]).unwrap();
        assert_eq!((p.new, p.uploads.len(), p.orphans.len()), (1, 1, 0));
        assert!(plan(&json!({ "photos": 3 }), &[], &[]).is_err());
    }
}
