//! The Google Docs / Drive client (DESIGN.md §6a.4): OAuth sign-in through
//! a loopback redirect, a cached refresh token, and the calls `wp` makes —
//! fetch a document, post a `batchUpdate`, list Drive. Every call blocks;
//! open and save run on the main thread, listings on a worker thread the
//! Open from Drive dialog spawns (DESIGN.md §6a.4).

use crate::config::{state_dir, GoogleConfig};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const SCOPES: &str = "https://www.googleapis.com/auth/documents https://www.googleapis.com/auth/drive.readonly";
const DOCS_URL: &str = "https://docs.googleapis.com/v1/documents";
const DRIVE_FILES_URL: &str = "https://www.googleapis.com/drive/v3/files";
const DRIVES_URL: &str = "https://www.googleapis.com/drive/v3/drives";
/// How long the sign-in page may take before `wp` gives up waiting.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Token {
    access_token: String,
    refresh_token: String,
    /// Unix seconds.
    expires_at: u64,
}

/// What a Drive row is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DriveKind {
    Doc,
    Folder,
    /// The "Shared with me" pseudo-folder.
    SharedWithMe,
    /// The "Shared drives" pseudo-folder; its entries are drives.
    SharedDrives,
}

/// A document, folder, or shared drive listed from Drive.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriveEntry {
    pub id: String,
    pub name: String,
    pub kind: DriveKind,
    /// Modified date, shown greyed to the right; empty for folders.
    pub detail: String,
}

/// One listing the dialog can ask for.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum DriveQuery {
    /// The three top-level places, listed locally.
    Roots,
    /// Google Docs by recency (last viewed, modified, or shared).
    Recent,
    /// Google Docs whose name contains the words.
    Search(String),
    /// Docs and folders inside a folder (`root` is My Drive; a shared
    /// drive's id is its root folder).
    Folder(String),
    SharedWithMe,
    SharedDrives,
}

const DOC_MIME: &str = "application/vnd.google-apps.document";
const FOLDER_MIME: &str = "application/vnd.google-apps.folder";

/// Why a request failed, as Google reported it.
#[derive(Debug)]
pub struct ApiError {
    pub status: u16,
    pub message: String,
}

impl ApiError {
    /// The `requiredRevisionId` guard fired: the document changed on Google's
    /// side since it was read.
    pub fn is_conflict(&self) -> bool {
        let m = self.message.to_ascii_lowercase();
        self.status == 400 && m.contains("revision")
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Google API {}: {}", self.status, self.message)
    }
}

impl std::error::Error for ApiError {}

/// A sign-in in progress: the URL the user must visit, and the listener the
/// redirect will land on.
pub struct SignIn {
    pub url: String,
    listener: TcpListener,
    redirect_uri: String,
    state: String,
}

/// Cloneable so a listing can run on a worker thread: the clone shares the
/// HTTP agent, and a token it refreshes is written to the same file.
#[derive(Clone)]
pub struct Client {
    cfg: GoogleConfig,
    agent: ureq::Agent,
    token: Option<Token>,
    token_path: PathBuf,
}

impl Client {
    pub fn new(cfg: GoogleConfig) -> Client {
        let config = ureq::Agent::config_builder().http_status_as_error(false).timeout_global(Some(Duration::from_secs(60))).build();
        let token_path = state_dir().join("google-token.json");
        let token = std::fs::read_to_string(&token_path).ok().and_then(|s| serde_json::from_str(&s).ok());
        Client { cfg, agent: ureq::Agent::new_with_config(config), token, token_path }
    }

    pub fn signed_in(&self) -> bool {
        self.token.as_ref().map_or(false, |t| !t.refresh_token.is_empty())
    }

    /// Forget the cached token; the next call signs in again.
    pub fn sign_out(&mut self) {
        self.token = None;
        let _ = std::fs::remove_file(&self.token_path);
    }

    /// Start the loopback flow: bind a local port and build the consent URL.
    /// The caller shows the URL (and opens it), then calls `finish_sign_in`.
    pub fn begin_sign_in(&self) -> anyhow::Result<SignIn> {
        if !self.cfg.is_set() {
            anyhow::bail!("no Google client id in config.toml — see [google] in the config file");
        }
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let redirect_uri = format!("http://127.0.0.1:{}", port);
        let state = random_token();
        let url = format!(
            "{}?client_id={}&redirect_uri={}&response_type=code&scope={}&access_type=offline&prompt=consent&state={}",
            AUTH_URL,
            urlencode(&self.cfg.client_id),
            urlencode(&redirect_uri),
            urlencode(SCOPES),
            state
        );
        Ok(SignIn { url, listener, redirect_uri, state })
    }

    /// Wait for the browser to come back with a code, then exchange it.
    /// `cancel` is polled while waiting so the user can give up.
    pub fn finish_sign_in(&mut self, flow: SignIn, mut cancel: impl FnMut() -> bool) -> anyhow::Result<()> {
        let started = Instant::now();
        let code = loop {
            match flow.listener.accept() {
                Ok((mut stream, _)) => {
                    let mut buf = [0u8; 4096];
                    let n = stream.read(&mut buf).unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let line = req.lines().next().unwrap_or("");
                    let path = line.split_whitespace().nth(1).unwrap_or("");
                    let query = path.splitn(2, '?').nth(1).unwrap_or("");
                    let params: Vec<(String, String)> = query
                        .split('&')
                        .filter_map(|kv| {
                            let (k, v) = kv.split_once('=')?;
                            Some((k.to_string(), urldecode(v)))
                        })
                        .collect();
                    let get = |k: &str| params.iter().find(|(pk, _)| pk == k).map(|(_, v)| v.clone());
                    let ok = get("state").as_deref() == Some(flow.state.as_str()) && get("code").is_some();
                    let body = if ok { "<!doctype html><title>wp</title><p style=\"font: 16px system-ui; margin: 3em\">Signed in. You can return to the terminal.</p>" } else { "<!doctype html><title>wp</title><p style=\"font: 16px system-ui; margin: 3em\">Sign-in did not complete. You can close this tab.</p>" };
                    let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
                    let _ = stream.flush();
                    if ok {
                        break get("code").unwrap();
                    }
                    if let Some(e) = get("error") {
                        anyhow::bail!("sign-in refused: {}", e);
                    }
                    // A favicon request or the like: keep waiting.
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if cancel() {
                        anyhow::bail!("sign-in cancelled");
                    }
                    if started.elapsed() > SIGN_IN_TIMEOUT {
                        anyhow::bail!("sign-in timed out");
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => return Err(e.into()),
            }
        };
        let v = self.token_request(&[
            ("code", code.as_str()),
            ("client_id", self.cfg.client_id.as_str()),
            ("client_secret", self.cfg.client_secret.as_str()),
            ("redirect_uri", flow.redirect_uri.as_str()),
            ("grant_type", "authorization_code"),
        ])?;
        let refresh = v.get("refresh_token").and_then(Value::as_str).unwrap_or("").to_string();
        if refresh.is_empty() {
            anyhow::bail!("Google did not return a refresh token; remove wp's access at myaccount.google.com/permissions and sign in again");
        }
        self.store_token(&v, refresh)?;
        Ok(())
    }

    fn token_request(&self, form: &[(&str, &str)]) -> anyhow::Result<Value> {
        let mut resp = self.agent.post(TOKEN_URL).send_form(form.iter().copied())?;
        let status = resp.status().as_u16();
        let text = resp.body_mut().read_to_string()?;
        let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if status != 200 {
            let msg = v.get("error_description").or_else(|| v.get("error")).and_then(Value::as_str).unwrap_or(&text).to_string();
            return Err(ApiError { status, message: msg }.into());
        }
        Ok(v)
    }

    fn store_token(&mut self, v: &Value, refresh_token: String) -> anyhow::Result<()> {
        let expires_in = v.get("expires_in").and_then(Value::as_u64).unwrap_or(3600);
        let t = Token { access_token: v.get("access_token").and_then(Value::as_str).unwrap_or("").to_string(), refresh_token, expires_at: now() + expires_in.saturating_sub(60) };
        if let Some(d) = self.token_path.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(&self.token_path, serde_json::to_string(&t)?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&self.token_path, std::fs::Permissions::from_mode(0o600));
        }
        self.token = Some(t);
        Ok(())
    }

    /// A valid access token, refreshed if the cached one has expired.
    fn access_token(&mut self) -> anyhow::Result<String> {
        let Some(t) = self.token.clone() else { anyhow::bail!("not signed in") };
        if now() < t.expires_at && !t.access_token.is_empty() {
            return Ok(t.access_token);
        }
        let v = self.token_request(&[
            ("refresh_token", t.refresh_token.as_str()),
            ("client_id", self.cfg.client_id.as_str()),
            ("client_secret", self.cfg.client_secret.as_str()),
            ("grant_type", "refresh_token"),
        ]);
        match v {
            Ok(v) => {
                self.store_token(&v, t.refresh_token)?;
                Ok(self.token.as_ref().unwrap().access_token.clone())
            }
            Err(e) => {
                // A revoked grant: sign in again next time.
                if e.downcast_ref::<ApiError>().map_or(false, |a| a.status == 400 || a.status == 401) {
                    self.sign_out();
                }
                Err(e)
            }
        }
    }

    fn call(&mut self, method: &str, url: &str, body: Option<&Value>) -> anyhow::Result<Value> {
        let tok = self.access_token()?;
        let auth = format!("Bearer {}", tok);
        let mut resp = match (method, body) {
            ("POST", Some(b)) => self.agent.post(url).header("Authorization", &auth).send_json(b)?,
            _ => self.agent.get(url).header("Authorization", &auth).call()?,
        };
        let status = resp.status().as_u16();
        let text = resp.body_mut().with_config().limit(256 * 1024 * 1024).read_to_string()?;
        if status < 200 || status >= 300 {
            let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            let message = v.get("error").and_then(|e| e.get("message")).and_then(Value::as_str).unwrap_or(&text).to_string();
            return Err(ApiError { status, message }.into());
        }
        Ok(serde_json::from_str(&text)?)
    }

    /// `documents.get`, as the JSON text `wp_gdoc::read` takes.
    pub fn get_document(&mut self, id: &str) -> anyhow::Result<String> {
        let v = self.call("GET", &format!("{}/{}", DOCS_URL, urlencode(id)), None)?;
        Ok(v.to_string())
    }

    /// `documents.batchUpdate`; returns the response (with the new
    /// `documentId` and per-request replies).
    pub fn batch_update(&mut self, id: &str, body: &Value) -> anyhow::Result<Value> {
        self.call("POST", &format!("{}/{}:batchUpdate", DOCS_URL, urlencode(id)), Some(body))
    }

    /// One Drive listing. `Roots` never reaches the network.
    pub fn list(&mut self, q: &DriveQuery) -> anyhow::Result<Vec<DriveEntry>> {
        let docs_and_folders = format!("(mimeType='{}' or mimeType='{}')", DOC_MIME, FOLDER_MIME);
        let (filter, order) = match q {
            DriveQuery::Roots => return Ok(drive_roots()),
            DriveQuery::Recent => (format!("mimeType='{}' and trashed=false", DOC_MIME), "recency desc"),
            DriveQuery::Search(words) => (format!("mimeType='{}' and trashed=false and name contains '{}'", DOC_MIME, drive_quote(words.trim())), "modifiedTime desc"),
            DriveQuery::Folder(id) => (format!("'{}' in parents and trashed=false and {}", drive_quote(id), docs_and_folders), "folder,name_natural"),
            DriveQuery::SharedWithMe => (format!("sharedWithMe=true and trashed=false and {}", docs_and_folders), "folder,name_natural"),
            DriveQuery::SharedDrives => {
                let v = self.call("GET", &format!("{}?pageSize=100&fields=drives(id,name)", DRIVES_URL), None)?;
                return Ok(json_list(&v, "drives")
                    .map(|d| DriveEntry { id: json_str(d, "id"), name: json_str(d, "name"), kind: DriveKind::Folder, detail: String::new() })
                    .collect());
            }
        };
        let url = format!(
            "{}?q={}&orderBy={}&pageSize=100&fields=files(id,name,mimeType,modifiedTime)&supportsAllDrives=true&includeItemsFromAllDrives=true",
            DRIVE_FILES_URL,
            urlencode(&filter),
            urlencode(order)
        );
        let v = self.call("GET", &url, None)?;
        Ok(json_list(&v, "files")
            .map(|f| {
                let folder = json_str(f, "mimeType") == FOLDER_MIME;
                DriveEntry {
                    id: json_str(f, "id"),
                    name: json_str(f, "name"),
                    kind: if folder { DriveKind::Folder } else { DriveKind::Doc },
                    detail: if folder { String::new() } else { json_str(f, "modifiedTime").chars().take(16).collect::<String>().replace('T', " ") },
                }
            })
            .collect())
    }
}

/// The top of the folder view.
pub fn drive_roots() -> Vec<DriveEntry> {
    vec![
        DriveEntry { id: "root".into(), name: "My Drive".into(), kind: DriveKind::Folder, detail: String::new() },
        DriveEntry { id: String::new(), name: "Shared with me".into(), kind: DriveKind::SharedWithMe, detail: String::new() },
        DriveEntry { id: String::new(), name: "Shared drives".into(), kind: DriveKind::SharedDrives, detail: String::new() },
    ]
}

/// The listing an entry opens onto, or None for a document.
pub fn query_for(e: &DriveEntry) -> Option<DriveQuery> {
    match e.kind {
        DriveKind::Doc => None,
        DriveKind::Folder => Some(DriveQuery::Folder(e.id.clone())),
        DriveKind::SharedWithMe => Some(DriveQuery::SharedWithMe),
        DriveKind::SharedDrives => Some(DriveQuery::SharedDrives),
    }
}

/// A string literal inside a Drive `q` expression.
fn drive_quote(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

fn json_list<'a>(v: &'a Value, key: &str) -> impl Iterator<Item = &'a Value> {
    v.get(key).and_then(Value::as_array).map(|a| a.iter()).into_iter().flatten()
}

fn json_str(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

/// A document id from `gdoc:<id>`, a Docs URL, or a bare id.
pub fn parse_doc_ref(s: &str) -> Option<String> {
    let s = s.trim();
    if let Some(id) = s.strip_prefix("gdoc:") {
        return Some(id.trim_matches('/').to_string()).filter(|i| !i.is_empty());
    }
    if let Some(rest) = s.strip_prefix("https://docs.google.com/document/d/").or_else(|| s.strip_prefix("http://docs.google.com/document/d/")) {
        let id: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
        return Some(id).filter(|i| !i.is_empty());
    }
    None
}

/// Open a URL in the user's browser, if the platform has a way to.
pub fn open_in_browser(url: &str) -> bool {
    let cmd = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    std::process::Command::new(cmd).arg(url).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().is_ok()
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn random_token() -> String {
    let mut bytes = [0u8; 24];
    let mut ok = false;
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        ok = f.read_exact(&mut bytes).is_ok();
    }
    if !ok {
        let t = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = (t >> (i * 5)) as u8 ^ (std::process::id() as u8).wrapping_mul(i as u8 + 1);
        }
    }
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

pub fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn urldecode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() + 0 && i + 2 <= bytes.len() - 1 => {
                if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                    out.push(v);
                    i += 3;
                    continue;
                }
                out.push(b'%');
                i += 1;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doc_refs() {
        assert_eq!(parse_doc_ref("gdoc:1AbC_d-e").as_deref(), Some("1AbC_d-e"));
        assert_eq!(parse_doc_ref("https://docs.google.com/document/d/1AbC_d-e/edit?tab=t.0#heading=h.1").as_deref(), Some("1AbC_d-e"));
        assert_eq!(parse_doc_ref("report.docx"), None);
        assert_eq!(parse_doc_ref("gdoc:"), None);
    }

    #[test]
    fn url_coding() {
        assert_eq!(urlencode("a b/c"), "a%20b%2Fc");
        assert_eq!(urldecode("a%20b%2Fc+d"), "a b/c d");
    }

    #[test]
    fn drive_quoting_and_roots() {
        assert_eq!(drive_quote("it's a \\ test"), "it\\'s a \\\\ test");
        let roots = drive_roots();
        assert_eq!(roots.len(), 3);
        assert_eq!(query_for(&roots[0]), Some(DriveQuery::Folder("root".into())));
        assert_eq!(query_for(&roots[1]), Some(DriveQuery::SharedWithMe));
        assert_eq!(query_for(&DriveEntry { id: "x".into(), name: "d".into(), kind: DriveKind::Doc, detail: String::new() }), None);
    }

    /// Against the real API with the user's cached token: `cargo test -p wp
    /// live_listings -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_listings() {
        let (cfg, _) = crate::config::Config::load();
        let mut c = Client::new(cfg.google);
        assert!(c.signed_in(), "not signed in");
        for q in [DriveQuery::Recent, DriveQuery::Search("a".into()), DriveQuery::Folder("root".into()), DriveQuery::SharedWithMe, DriveQuery::SharedDrives] {
            let t = Instant::now();
            let rows = c.list(&q).unwrap_or_else(|e| panic!("{:?}: {}", q, e));
            println!("{:?}: {} rows in {:?}; first: {:?}", q, rows.len(), t.elapsed(), rows.first().map(|r| (&r.name, r.kind, &r.detail)));
            if let Some(f) = rows.iter().find(|r| r.kind == DriveKind::Folder) {
                if matches!(q, DriveQuery::Folder(_)) {
                    let sub = c.list(&DriveQuery::Folder(f.id.clone())).unwrap();
                    println!("  {}/: {} rows", f.name, sub.len());
                }
            }
        }
    }

    #[test]
    fn conflict_detection() {
        assert!(ApiError { status: 400, message: "The document revision is not the latest".into() }.is_conflict());
        assert!(!ApiError { status: 404, message: "Requested entity was not found".into() }.is_conflict());
    }
}
