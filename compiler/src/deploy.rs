// `plumb login`, `plumb deploy`, `plumb secret`: a project's apps on a
// wasm-server, through its admin API (/_platform/). What runs and how comes
// from plumb.toml's [project] and [apps.*], or [app] (project::read_project);
// the server's admin token is
// kept per server in ~/.config/plumb/servers, readable by the owner only,
// or comes from PLUMB_TOKEN. Requests go through curl, which every system
// we build on has; the token goes to curl on its standard input, never on
// the command line.

use crate::project::{self, AppConfig, ProjectConfig};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

fn credentials_file() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".config").join("plumb").join("servers")
}

fn saved_token(server: &str) -> Option<String> {
    let text = std::fs::read_to_string(credentials_file()).ok()?;
    text.lines().find_map(|l| {
        let (url, token) = l.split_once(' ')?;
        (url == server).then(|| token.trim().to_string())
    })
}

fn save_token(server: &str, token: &str) -> Result<(), String> {
    let file = credentials_file();
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
    }
    let old = std::fs::read_to_string(&file).unwrap_or_default();
    let mut text: String = old.lines().filter(|l| l.split(' ').next() != Some(server)).map(|l| format!("{}\n", l)).collect();
    text += &format!("{} {}\n", server, token);
    let tmp = file.with_extension("tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("{}: {}", tmp.display(), e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, &file).map_err(|e| format!("{}: {}", file.display(), e))
}

fn token_for(server: &str) -> Result<String, String> {
    if let Ok(t) = std::env::var("PLUMB_TOKEN") {
        return Ok(t);
    }
    saved_token(server).ok_or_else(|| format!("no token for {}: run `plumb login {}` (or set PLUMB_TOKEN)", server, server))
}

/// One request to the server's admin API: (status, body).
fn request(method: &str, url: &str, token: &str, body: Option<&Path>) -> Result<(u16, String), String> {
    let mut cmd = Command::new("curl");
    cmd.args(["-sS", "-K", "-", "-X", method, "-w", "\n%{http_code}"]);
    if let Some(file) = body {
        cmd.args(["-H", "content-type: application/json", "--data-binary"]).arg(format!("@{}", file.display()));
    }
    cmd.arg(url).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("can't run curl: {}", e))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = writeln!(stdin, "header = \"Authorization: Bearer {}\"", token.replace('\\', "\\\\").replace('"', "\\\""));
    }
    let out = child.wait_with_output().map_err(|e| format!("curl: {}", e))?;
    if !out.status.success() {
        return Err(format!("can't reach {}: {}", url, String::from_utf8_lossy(&out.stderr).trim()));
    }
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let (body, code) = text.rsplit_once('\n').unwrap_or(("", &text));
    Ok((code.trim().parse().unwrap_or(0), body.to_string()))
}

/// The server's `{"error": "..."}`, or the body as it is.
fn server_error(body: &str) -> String {
    let b = body.trim();
    if let Some(rest) = b.strip_prefix("{\"error\":\"") {
        if let Some(msg) = rest.strip_suffix("\"}") {
            return msg.replace("\\\"", "\"").replace("\\n", "\n");
        }
    }
    b.to_string()
}

fn json_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out += "\\\"",
            '\\' => out += "\\\\",
            '\n' => out += "\\n",
            '\r' => out += "\\r",
            '\t' => out += "\\t",
            c if (c as u32) < 0x20 => out += &format!("\\u{:04x}", c as u32),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn json_list(items: &[String]) -> String {
    format!("[{}]", items.iter().map(|s| json_str(s)).collect::<Vec<_>>().join(","))
}


fn fail(msg: String) -> ExitCode {
    eprintln!("error: {}", msg);
    ExitCode::from(1)
}

fn project_app() -> Result<(PathBuf, ProjectConfig), String> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let root = project::find_root(&cwd.join("x.plumb"));
    let app = project::read_project(&root)?;
    Ok((root, app))
}

/// The project's apps named in `args` (words not starting with `-` and not
/// a flag's value), or all of them.
fn chosen_apps<'a>(p: &'a ProjectConfig, args: &[String], flags_with_value: &[&str]) -> Result<Vec<&'a AppConfig>, String> {
    let mut names = vec![];
    let mut i = 0;
    while i < args.len() {
        if flags_with_value.contains(&args[i].as_str()) {
            i += 2;
            continue;
        }
        if !args[i].starts_with('-') {
            names.push(args[i].clone());
        }
        i += 1;
    }
    if names.is_empty() {
        return Ok(p.apps.iter().collect());
    }
    names
        .iter()
        .map(|n| {
            p.apps.iter().find(|a| &a.name == n).ok_or_else(|| {
                let all: Vec<&str> = p.apps.iter().map(|a| a.name.as_str()).collect();
                format!("the project has no app {:?}; its apps: {}", n, all.join(", "))
            })
        })
        .collect()
}

/// Where an app answers: its domain and path, or `base` with its name.
fn place(p: &ProjectConfig, app: &AppConfig, base: &str, local: bool) -> String {
    let path = app.path.as_deref().unwrap_or("/");
    match p.domains.first() {
        Some(d) if !local => format!("https://{}{}", d, path),
        _ if local => format!("{}{}", base, path),
        _ => format!("{}/{}", base, p.server_name(app)),
    }
}

fn server_of(app: &ProjectConfig, args: &[String]) -> Result<String, String> {
    if let Some(i) = args.iter().position(|a| a == "--to") {
        return args.get(i + 1).map(|s| s.trim_end_matches('/').to_string()).ok_or_else(|| "--to needs the server's URL".to_string());
    }
    app.server.clone().ok_or_else(|| "where to? add the server to plumb.toml:\n\n[deploy]\nserver = \"https://my-server.example.com\"\n\nor give it: plumb deploy --to https://...".to_string())
}

/// `plumb login <server>`: asks for the admin token, checks it, keeps it.
pub fn login(args: &[String]) -> ExitCode {
    let Some(server) = args.first().map(|s| s.trim_end_matches('/').to_string()) else {
        return fail("usage: plumb login <server URL>, e.g. plumb login https://my-server.example.com".into());
    };
    eprint!("the server's admin token (--admin-token): ");
    let mut token = String::new();
    if std::io::stdin().read_line(&mut token).is_err() || token.trim().is_empty() {
        return fail("no token given".into());
    }
    let token = token.trim();
    match request("GET", &format!("{}/_platform/whoami", server), token, None) {
        Ok((200, body)) if body.contains("\"admin\"") => {}
        Ok((200 | 401 | 403, _)) => return fail(format!("{} doesn't accept this token as its admin token", server)),
        Ok((code, body)) => return fail(format!("{} answered {}: {}", server, code, server_error(&body))),
        Err(e) => return fail(e),
    }
    match save_token(&server, token) {
        Ok(()) => {
            println!("logged in to {} (the token is in {})", server, credentials_file().display());
            ExitCode::SUCCESS
        }
        Err(e) => fail(e),
    }
}

/// Runs each app's `build` command (making its static files), in the
/// project's directory. Err(""): one failed and said why.
fn build_static(root: &Path, p: &ProjectConfig) -> Result<(), String> {
    for app in &p.apps {
        if let Some(cmd) = &app.build {
            eprintln!("{}: {}", app.name, cmd);
            let ok = Command::new("sh").arg("-c").arg(cmd).current_dir(root).status().map_err(|e| format!("can't run sh: {}", e))?.success();
            if !ok {
                eprintln!("error: {}: `{}` failed", app.name, cmd);
                return Err(String::new());
            }
        }
    }
    Ok(())
}

/// The files of a static directory: (path inside it, absolute path),
/// without dot files.
fn static_files(dir: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> Result<(), String> {
        let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
        for e in entries.flatten() {
            let path = e.path();
            if e.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            if path.is_dir() {
                walk(root, &path, out)?;
            } else {
                let rel = path.strip_prefix(root).unwrap().components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect::<Vec<_>>().join("/");
                out.push((rel, path));
            }
        }
        Ok(())
    }
    if !dir.is_dir() {
        return Err(format!("{} isn't there: the static files of an app (did its build run?)", dir.display()));
    }
    let mut out = vec![];
    walk(dir, dir, &mut out)?;
    out.sort();
    if out.is_empty() {
        return Err(format!("{} has no files", dir.display()));
    }
    Ok(out)
}

/// A release made locally: its manifest, and the parts it names by hash.
struct Built {
    manifest: String,
    parts: Vec<(String, Vec<u8>)>,
}

/// Builds the project's apps and gathers their files and schema into a
/// release; `extra_domains` are served too (`plumb dev`: localhost).
/// Err(""): a build failed and said why.
fn build_release(root: &Path, p: &ProjectConfig, extra_domains: &[&str], build: &dyn Fn(&Path, &Path) -> bool) -> Result<Built, String> {
    let mut parts: Vec<(String, Vec<u8>)> = vec![];
    let mut add = |data: Vec<u8>| -> String {
        let h = sha256_hex(&data);
        if !parts.iter().any(|(k, _)| *k == h) {
            parts.push((h.clone(), data));
        }
        h
    };
    let (mut components, mut files, mut apps) = (vec![], vec![], vec![]);
    for app in &p.apps {
        if let Some(main) = &app.main {
            let main = root.join(main);
            let wasm = std::env::temp_dir().join(format!("plumb-deploy-{}-{}-{}.wasm", p.name, app.name, std::process::id()));
            if p.apps.len() > 1 {
                eprintln!("building {}", app.name);
            }
            if !build(&main, &wasm) {
                return Err(String::new());
            }
            let component = std::fs::read(&wasm).map_err(|e| format!("{}: {}", wasm.display(), e))?;
            let _ = std::fs::remove_file(&wasm);
            components.push(format!("{}:\"{}\"", json_str(&app.name), add(component)));
        }
        if let Some(dir) = &app.static_dir {
            let mut list = vec![];
            for (rel, path) in static_files(&root.join(dir))? {
                let data = std::fs::read(&path).map_err(|e| format!("{}: {}", path.display(), e))?;
                list.push(format!("{}:\"{}\"", json_str(&rel), add(data)));
            }
            files.push(format!("{}:{{{}}}", json_str(&app.name), list.join(",")));
        }
        let mut settings = vec![];
        if let Some(path) = &app.path {
            settings.push(format!("\"path\":{}", json_str(path)));
        }
        if let Some(w) = &app.websocket {
            settings.push(format!("\"websocket\":{}", json_str(w)));
        }
        if let Some(o) = &app.outbound {
            settings.push(format!("\"outbound\":{}", json_list(o)));
        }
        if app.spa {
            settings.push("\"spa\":true".to_string());
        }
        apps.push(format!("{}:{{{}}}", json_str(&app.name), settings.join(",")));
    }
    let mut schema = vec![];
    for (db, file) in &p.schema {
        let sql = std::fs::read(root.join(file)).map_err(|e| format!("the schema of {}: {}: {}", db, root.join(file).display(), e))?;
        schema.push(format!("{}:\"{}\"", json_str(db), add(sql)));
    }
    let mut domains = p.domains.clone();
    domains.extend(extra_domains.iter().map(|d| d.to_string()));
    let mut settings = vec![format!("\"domains\":{}", json_list(&domains)), format!("\"databases\":{}", json_list(&p.databases))];
    if let Some(o) = &p.outbound {
        settings.push(format!("\"outbound\":{}", json_list(o)));
    }
    settings.push(format!("\"apps\":{{{}}}", apps.join(",")));
    let manifest = format!(
        "{{\"project\":{},\"settings\":{{{}}},\"components\":{{{}}},\"files\":{{{}}},\"schema\":{{{}}},\"compiler\":{}}}",
        json_str(&p.name),
        settings.join(","),
        components.join(","),
        files.join(","),
        schema.join(","),
        json_str(&format!("plumb {}", env!("CARGO_PKG_VERSION")))
    );
    Ok(Built { manifest, parts })
}

/// What a deploy did.
struct Pushed {
    release: String,
    uploaded: usize,
    uploaded_bytes: usize,
    total: usize,
    unchanged: bool,
}

/// The strings of a JSON array under `key` (`{"missing": ["ab", ...]}`).
fn json_strings(body: &str, key: &str) -> Vec<String> {
    let Some(at) = body.find(&format!("\"{}\":[", key)) else { return vec![] };
    let rest = &body[at + key.len() + 4..];
    let inner = &rest[..rest.find(']').unwrap_or(rest.len())];
    inner.split(',').map(|s| s.trim().trim_matches('"').to_string()).filter(|s| !s.is_empty()).collect()
}

/// A string field of a flat JSON object.
fn json_field(body: &str, key: &str) -> Option<String> {
    let at = body.find(&format!("\"{}\":\"", key))? + key.len() + 4;
    let rest = &body[at..];
    Some(rest[..rest.find('"')?].to_string())
}

/// Sends a release: asks which parts the server lacks, uploads only those
/// (in one request), then the manifest, which the server switches to.
fn push(server: &str, token: &str, project: &str, built: &Built, note: &str) -> Result<Pushed, String> {
    let tmp = std::env::temp_dir().join(format!("plumb-push-{}", std::process::id()));
    let hashes: Vec<String> = built.parts.iter().map(|(h, _)| h.clone()).collect();
    std::fs::write(&tmp, format!("{{\"hashes\":{}}}", json_list(&hashes))).map_err(|e| e.to_string())?;
    let missing = match request("POST", &format!("{}/_platform/blobs/missing", server), token, Some(&tmp)) {
        Ok((200, body)) => json_strings(&body, "missing"),
        Ok((code, body)) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("{} ({})", server_error(&body), code));
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
    };
    let mut batch: Vec<u8> = vec![];
    let mut uploaded = 0;
    for (h, data) in built.parts.iter().filter(|(h, _)| missing.contains(h)) {
        batch.extend_from_slice(format!("{} {}\n", h, data.len()).as_bytes());
        batch.extend_from_slice(data);
        uploaded += 1;
    }
    let uploaded_bytes = batch.len();
    if uploaded > 0 {
        std::fs::write(&tmp, &batch).map_err(|e| e.to_string())?;
        match request("POST", &format!("{}/_platform/blobs", server), token, Some(&tmp)) {
            Ok((200, _)) => {}
            Ok((code, body)) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(format!("uploading: {} ({})", server_error(&body), code));
            }
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(e);
            }
        }
    }
    std::fs::write(&tmp, format!("{{\"release\":{},\"note\":{}}}", built.manifest, json_str(note))).map_err(|e| e.to_string())?;
    let result = request("POST", &format!("{}/_platform/projects/{}/releases", server, project), token, Some(&tmp));
    let _ = std::fs::remove_file(&tmp);
    match result {
        Ok((200, body)) => Ok(Pushed {
            release: json_field(&body, "release").unwrap_or_default(),
            uploaded,
            uploaded_bytes,
            total: built.parts.len(),
            unchanged: body.contains("\"unchanged\":true"),
        }),
        Ok((code, body)) => Err(format!("{} ({})", server_error(&body), code)),
        Err(e) => Err(e),
    }
}

/// What to note with a release: `-m TEXT`, or the git commit (with
/// "+ changes" when the tree has uncommitted ones).
fn release_note(root: &Path, args: &[String]) -> String {
    if let Some(m) = args.iter().position(|a| a == "-m").and_then(|i| args.get(i + 1)) {
        return m.clone();
    }
    let git = |a: &[&str]| Command::new("git").args(a).current_dir(root).stderr(Stdio::null()).output().ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    match git(&["log", "-1", "--format=%h %s"]) {
        Some(c) if !c.is_empty() => {
            let dirty = git(&["status", "--porcelain"]).is_some_and(|s| !s.is_empty());
            if dirty { format!("{} + changes", c) } else { c }
        }
        _ => String::new(),
    }
}

/// SHA-256 (FIPS 180-4), hex: the store's names for parts.
fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[i * 4], chunk[i * 4 + 1], chunk[i * 4 + 2], chunk[i * 4 + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let mut v = h;
        for i in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7].wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v = [t1.wrapping_add(t2), v[0], v[1], v[2], v[3].wrapping_add(t1), v[4], v[5], v[6]];
        }
        for i in 0..8 {
            h[i] = h[i].wrapping_add(v[i]);
        }
    }
    h.iter().map(|x| format!("{:08x}", x)).collect()
}

/// `plumb deploy [--to URL]`: builds the project's apps for wasm-server and
/// sends them with their settings and schema; the server switches to all of
/// them at once, without downtime, or (if one doesn't start) to none.
pub fn deploy(args: &[String], build: &dyn Fn(&Path, &Path) -> bool) -> ExitCode {
    let (root, app) = match project_app() {
        Ok(x) => x,
        Err(e) => return fail(e),
    };
    let server = match server_of(&app, args) {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    let token = match token_for(&server) {
        Ok(t) => t,
        Err(e) => return fail(e),
    };
    if let Err(e) = build_static(&root, &app) {
        return if e.is_empty() { ExitCode::from(1) } else { fail(e) };
    }
    let built = match build_release(&root, &app, &[], build) {
        Ok(x) => x,
        Err(e) if e.is_empty() => return ExitCode::from(1),
        Err(e) => return fail(e),
    };
    let note = release_note(&root, args);
    eprintln!("deploying {} to {}", app.name, server);
    match push(&server, &token, &app.name, &built, &note) {
        Ok(p) => {
            let sent = if p.uploaded == 0 { "nothing new to upload".to_string() } else { format!("uploaded {} of {} parts, {} KB", p.uploaded, p.total, p.uploaded_bytes.div_ceil(1024)) };
            let what = if p.unchanged { "already running" } else { "deployed" };
            println!("{} {} (release {}; {})", what, app.name, p.release.get(..12).unwrap_or(""), sent);
            for a in &app.apps {
                println!("  {:<12} {}", a.name, place(&app, a, &server, false));
            }
            ExitCode::SUCCESS
        }
        Err(e) => fail(format!("the server refused the deploy: {}", e)),
    }
}

/// `plumb releases`: what the project ran on its server, newest first.
pub fn releases(args: &[String]) -> ExitCode {
    let (_, project) = match project_app() {
        Ok(x) => x,
        Err(e) => return fail(e),
    };
    let server = match server_of(&project, args) {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    let token = match token_for(&server) {
        Ok(t) => t,
        Err(e) => return fail(e),
    };
    let body = match request("GET", &format!("{}/_platform/projects/{}/releases", server, project.name), &token, None) {
        Ok((200, b)) => b,
        Ok((code, b)) => return fail(format!("{} ({})", server_error(&b), code)),
        Err(e) => return fail(e),
    };
    let current = json_field(&body, "current").unwrap_or_default();
    let list = body.split("\"releases\":[").nth(1).unwrap_or("");
    for entry in list.split("},{") {
        let (Some(id), at) = (json_field(entry, "id"), entry.find("\"at_ms\":").map(|i| &entry[i + 8..])) else { continue };
        let ms: u64 = at.unwrap_or("0").chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().unwrap_or(0);
        let note = json_field(entry, "note").unwrap_or_default();
        let mark = if id == current { "→" } else { " " };
        println!("{} {}  {}  {}", mark, &id[..12.min(id.len())], ago(ms), note);
    }
    ExitCode::SUCCESS
}

/// "5 min ago", "3 days ago".
fn ago(ms: u64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    let s = now.saturating_sub(ms) / 1000;
    match s {
        0..=59 => format!("{:>3} s ago  ", s),
        60..=3599 => format!("{:>3} min ago", s / 60),
        3600..=86399 => format!("{:>3} h ago  ", s / 3600),
        _ => format!("{:>3} days ago", s / 86400),
    }
}

/// `plumb rollback [RELEASE]`: back to the release before the current one,
/// or to the one given (its first characters are enough).
pub fn rollback(args: &[String]) -> ExitCode {
    let (_, project) = match project_app() {
        Ok(x) => x,
        Err(e) => return fail(e),
    };
    let server = match server_of(&project, args) {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    let token = match token_for(&server) {
        Ok(t) => t,
        Err(e) => return fail(e),
    };
    let to = args.iter().enumerate().find(|(i, a)| !a.starts_with('-') && (*i == 0 || args[i - 1] != "--to")).map(|(_, a)| format!("?to={}", a)).unwrap_or_default();
    match request("POST", &format!("{}/_platform/projects/{}/rollback{}", server, project.name, to), &token, None) {
        Ok((200, body)) => {
            println!("{} runs release {} again", project.name, json_field(&body, "release").unwrap_or_default().get(..12).unwrap_or(""));
            ExitCode::SUCCESS
        }
        Ok((code, body)) => fail(format!("{} ({})", server_error(&body), code)),
        Err(e) => fail(e),
    }
}

/// `plumb secret set KEY=VALUE...`, `plumb secret set KEY` (the value from
/// standard input), `plumb secret unset KEY...`, `plumb secret list`.
pub fn secret(args: &[String]) -> ExitCode {
    let usage = "usage: plumb secret set KEY=VALUE... | plumb secret set KEY (reads the value) | plumb secret unset KEY... | plumb secret list";
    let (_, app) = match project_app() {
        Ok(x) => x,
        Err(e) => return fail(e),
    };
    // the words after the action, without `--to URL`
    let mut rest: Vec<String> = vec![];
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--to" {
            i += 2;
            continue;
        }
        rest.push(args[i].clone());
        i += 1;
    }
    let server = match server_of(&app, args) {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    let token = match token_for(&server) {
        Ok(t) => t,
        Err(e) => return fail(e),
    };
    let url = format!("{}/_platform/projects/{}/secrets", server, app.name);
    let changes: Vec<String> = match args.first().map(String::as_str) {
        Some("list") => {
            return match request("GET", &url, &token, None) {
                Ok((200, body)) => {
                    let names = body.split('[').nth(1).and_then(|s| s.split(']').next()).unwrap_or("");
                    for n in names.split(',').map(|s| s.trim().trim_matches('"')).filter(|s| !s.is_empty()) {
                        println!("{}", n);
                    }
                    ExitCode::SUCCESS
                }
                Ok((code, body)) => fail(format!("{} ({})", server_error(&body), code)),
                Err(e) => fail(e),
            };
        }
        Some("set") if !rest.is_empty() => {
            let mut out = vec![];
            for a in &rest {
                match a.split_once('=') {
                    Some((k, v)) => out.push(format!("{}:{}", json_str(k), json_str(v))),
                    None => {
                        eprint!("{}: ", a);
                        let mut v = String::new();
                        let _ = std::io::stdin().read_line(&mut v);
                        out.push(format!("{}:{}", json_str(a), json_str(v.trim_end_matches(['\n', '\r']))));
                    }
                }
            }
            out
        }
        Some("unset") if !rest.is_empty() => rest.iter().map(|k| format!("{}:null", json_str(k))).collect(),
        _ => return fail(usage.into()),
    };
    let body_file = std::env::temp_dir().join(format!("plumb-secret-{}.json", std::process::id()));
    if let Err(e) = std::fs::write(&body_file, format!("{{{}}}", changes.join(","))) {
        return fail(e.to_string());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&body_file, std::fs::Permissions::from_mode(0o600));
    }
    let result = request("PUT", &url, &token, Some(&body_file));
    let _ = std::fs::remove_file(&body_file);
    match result {
        Ok((200, _)) => {
            println!("done; {} restarted with them", if app.apps.len() > 1 { format!("{}'s apps", app.name) } else { app.name.clone() });
            ExitCode::SUCCESS
        }
        Ok((code, body)) => fail(format!("{} ({})", server_error(&body), code)),
        Err(e) => fail(e),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn sha256() {
        assert_eq!(super::sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(super::sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let long = vec![b'a'; 1000];
        assert_eq!(super::sha256_hex(&long), "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3");
    }
}

// ---- plumb dev

/// The wasm-server program: $WASM_SERVER, or `wasm-server` on the PATH.
fn wasm_server_bin() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("WASM_SERVER") {
        return Some(PathBuf::from(p));
    }
    std::env::var("PATH").ok()?.split(':').map(|d| Path::new(d).join("wasm-server")).find(|p| p.exists())
}

/// KEY=VALUE lines of the project's `.env` (secrets for local runs; keep it
/// out of git), as a JSON object for the secrets endpoint.
fn dotenv_json(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(root.join(".env")).ok()?;
    let mut pairs = vec![];
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        if let Some((k, v)) = line.split_once('=') {
            let v = v.trim();
            let v = v.strip_prefix('"').and_then(|x| x.strip_suffix('"')).or_else(|| v.strip_prefix('\'').and_then(|x| x.strip_suffix('\''))).unwrap_or(v);
            pairs.push(format!("{}:{}", json_str(k.trim()), json_str(v)));
        }
    }
    Some(format!("{{{}}}", pairs.join(",")))
}

/// The newest change among the project's files: code, schema, settings,
/// and every file in the apps' static directories.
fn newest_change(root: &Path, p: &ProjectConfig) -> std::time::SystemTime {
    fn walk(dir: &Path, newest: &mut std::time::SystemTime, depth: u32, all: bool) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for e in entries.flatten() {
            let path = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if (name.starts_with('.') && name != ".env") || name == "node_modules" {
                continue;
            }
            if path.is_dir() {
                if depth < 6 || all {
                    walk(&path, newest, depth + 1, all);
                }
            } else if all || name.ends_with(".plumb") || name.ends_with(".sql") || name == "plumb.toml" || name == ".env" {
                if let Ok(t) = e.metadata().and_then(|m| m.modified()) {
                    if t > *newest {
                        *newest = t;
                    }
                }
            }
        }
    }
    let mut newest = std::time::SystemTime::UNIX_EPOCH;
    walk(root, &mut newest, 0, false);
    for dir in p.apps.iter().filter_map(|a| a.static_dir.as_ref()) {
        walk(&root.join(dir), &mut newest, 0, true);
    }
    newest
}

/// `plumb dev [--port N]`: the project on a local wasm-server, deployed the
/// way `plumb deploy` does it (same settings, schema, databases, paths and
/// limits; localhost is one more domain; static files' `build` runs once,
/// then the files are watched),
/// with secrets from `.env`; rebuilt and redeployed when a file changes.
/// Its data stays in `.plumb/dev`.
pub fn dev(args: &[String], build: &dyn Fn(&Path, &Path) -> bool) -> ExitCode {
    let (root, mut app) = match project_app() {
        Ok(x) => x,
        Err(e) => return fail(e),
    };
    let port = args.iter().position(|a| a == "--port").and_then(|i| args.get(i + 1)).and_then(|p| p.parse::<u16>().ok()).unwrap_or(8080);
    let Some(bin) = wasm_server_bin() else {
        return fail("plumb dev runs the app on wasm-server: put `wasm-server` on the PATH, or set WASM_SERVER to it".into());
    };
    let data = root.join(".plumb").join("dev");
    if let Err(e) = std::fs::create_dir_all(&data) {
        return fail(format!("{}: {}", data.display(), e));
    }
    let token = format!("dev-{:x}{:x}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
    // the frontend's build runs once; after that its files are watched (run
    // its own watcher, `vite build --watch`, to rebuild them on changes)
    if let Err(e) = build_static(&root, &app) {
        return if e.is_empty() { ExitCode::from(1) } else { fail(e) };
    }
    let mut server = match Command::new(&bin)
        .args(["serve", "--listen", &format!("127.0.0.1:{}", port), "--workers", "2", "--admin-token", &token, "--data-dir"])
        .arg(&data)
        .stdin(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return fail(format!("can't start {}: {}", bin.display(), e)),
    };
    let base = format!("http://127.0.0.1:{}", port);
    let mut up = false;
    for _ in 0..100 {
        if let Ok((200, _)) = request("GET", &format!("{}/_platform/whoami", base), &token, None) {
            up = true;
            break;
        }
        if let Ok(Some(_)) = server.try_wait() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    if !up {
        let _ = server.kill();
        return fail(format!("wasm-server didn't start on {} (is the port taken? --port N)", base));
    }
    let mut seen = std::time::SystemTime::UNIX_EPOCH;
    let mut first = true;
    loop {
        let changed = newest_change(&root, &app);
        if changed > seen {
            seen = changed;
            if !first {
                eprintln!("\n-- changed: rebuilding");
                match project::read_project(&root) {
                    Ok(a) => app = a,
                    Err(e) => eprintln!("error: {}", e),
                }
            }
            match build_release(&root, &app, &["localhost", "127.0.0.1"], build) {
                Ok(built) => {
                    match push(&base, &token, &app.name, &built, "plumb dev") {
                        Ok(_) => {
                            if let Some(env) = dotenv_json(&root) {
                                let file = root.join(".plumb").join("dev").join("env.json");
                                if std::fs::write(&file, env).is_ok() {
                                    if let Ok((code, b)) = request("PUT", &format!("{}/_platform/projects/{}/secrets", base, app.name), &token, Some(&file)) {
                                        if code != 200 {
                                            eprintln!("error: .env: {}", server_error(&b));
                                        }
                                    }
                                    let _ = std::fs::remove_file(&file);
                                }
                            }
                            if first {
                                let local = format!("http://localhost:{}", port);
                                if let [one] = app.apps.as_slice() {
                                    eprintln!("{} is running on {} (data in {}); Ctrl-C stops it", app.name, place(&app, one, &local, true), data.display());
                                } else {
                                    eprintln!("{} is running (data in {}); Ctrl-C stops it", app.name, data.display());
                                    for a in &app.apps {
                                        eprintln!("  {:<12} {}", a.name, place(&app, a, &local, true));
                                    }
                                }
                            } else {
                                eprintln!("-- {} is running the new version", app.name);
                            }
                        }
                        Err(e) => eprintln!("error: the new version doesn't start: {}", e),
                    }
                }
                Err(e) if e.is_empty() => eprintln!("-- the build failed; the previous version keeps running"),
                Err(e) => eprintln!("error: {}", e),
            }
            first = false;
        }
        if let Ok(Some(status)) = server.try_wait() {
            return if status.success() { ExitCode::SUCCESS } else { fail(format!("wasm-server stopped ({})", status)) };
        }
        std::thread::sleep(std::time::Duration::from_millis(400));
    }
}

// ---- plumb logs, plumb stats

/// `plumb logs [APP...] [-n N] [-f]`: the log of the project's apps (or
/// the ones named) on their server, merged by time, each line marked with
/// its app when there are several; -f keeps printing what comes.
pub fn logs(args: &[String]) -> ExitCode {
    let (_, project) = match project_app() {
        Ok(x) => x,
        Err(e) => return fail(e),
    };
    let apps = match chosen_apps(&project, args, &["-n", "--to"]) {
        Ok(a) => a,
        Err(e) => return fail(e),
    };
    let server = match server_of(&project, args) {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    let token = match token_for(&server) {
        Ok(t) => t,
        Err(e) => return fail(e),
    };
    let lines = args.iter().position(|a| a == "-n").and_then(|i| args.get(i + 1)).and_then(|n| n.parse::<usize>().ok()).unwrap_or(100);
    let follow = args.iter().any(|a| a == "-f" || a == "--follow");
    let mark = apps.len() > 1;
    let width = apps.iter().map(|a| a.name.len()).max().unwrap_or(0);
    let mut offsets: Vec<Option<u64>> = vec![None; apps.len()];
    loop {
        let mut out: Vec<String> = vec![];
        for (i, app) in apps.iter().enumerate() {
            let base = format!("{}/_platform/apps/{}/logs", server, project.server_name(app));
            let url = match offsets[i] {
                Some(o) => format!("{}?since={}", base, o),
                None => format!("{}?lines={}", base, lines),
            };
            let headers = std::env::temp_dir().join(format!("plumb-logs-{}.headers", std::process::id()));
            let result = request_saving_headers(&url, &token, &headers);
            let offset = std::fs::read_to_string(&headers).ok().and_then(|h| {
                h.lines().find_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    k.trim().eq_ignore_ascii_case("x-log-offset").then(|| v.trim().parse::<u64>().ok())?
                })
            });
            let _ = std::fs::remove_file(&headers);
            match result {
                Ok((200, body)) => {
                    for line in body.lines() {
                        out.push(if mark { mark_line(line, &app.name, width) } else { line.to_string() });
                    }
                }
                Ok((code, body)) => return fail(format!("{} ({})", server_error(&body), code)),
                Err(e) => return fail(e),
            }
            offsets[i] = Some(offset.unwrap_or(0));
        }
        // lines start with their time (2026-10-13T08:15:02...): merge by the
        // second, keeping each app's order within it
        if mark {
            out.sort_by(|a, b| a.get(..19).unwrap_or(a).cmp(b.get(..19).unwrap_or(b)));
        }
        let skip = if follow && offsets.iter().all(Option::is_some) && out.len() <= lines { 0 } else { out.len().saturating_sub(lines) };
        for line in &out[skip..] {
            println!("{}", line);
        }
        if !follow {
            return ExitCode::SUCCESS;
        }
        let _ = std::io::stdout().flush();
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

/// A log line with its app's name after its time: `TIME api   | text`.
fn mark_line(line: &str, app: &str, width: usize) -> String {
    match line.split_once(' ') {
        Some((time, rest)) if time.len() > 10 && time.as_bytes()[10] == b'T' => format!("{} {:<w$} | {}", time, app, rest, w = width),
        _ => format!("{:<w$} | {}", app, line, w = width),
    }
}

/// `request`, also writing the response's headers to `headers`.
fn request_saving_headers(url: &str, token: &str, headers: &Path) -> Result<(u16, String), String> {
    let mut cmd = Command::new("curl");
    cmd.args(["-sS", "-K", "-", "-D"]).arg(headers).args(["-w", "\n%{http_code}"]).arg(url);
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("can't run curl: {}", e))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = writeln!(stdin, "header = \"Authorization: Bearer {}\"", token.replace('\\', "\\\\").replace('"', "\\\""));
    }
    let out = child.wait_with_output().map_err(|e| format!("curl: {}", e))?;
    if !out.status.success() {
        return Err(format!("can't reach {}: {}", url, String::from_utf8_lossy(&out.stderr).trim()));
    }
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let (body, code) = text.rsplit_once('\n').unwrap_or(("", &text));
    Ok((code.trim().parse().unwrap_or(0), body.to_string()))
}

/// A number field of a flat JSON object (the metrics' summary).
fn json_number(obj: &str, key: &str) -> Option<f64> {
    let at = obj.find(&format!("\"{}\":", key))? + key.len() + 3;
    let rest = &obj[at..];
    let end = rest.find([',', '}']).unwrap_or(rest.len());
    rest[..end].trim().parse().ok()
}

/// `plumb stats [APP] [--range 1h|6h|24h|7d|30d]`: requests, errors,
/// response times and where they go, from the server's metrics: the whole
/// project's, then each app's in a line (or one app's).
pub fn stats(args: &[String]) -> ExitCode {
    let (_, project) = match project_app() {
        Ok(x) => x,
        Err(e) => return fail(e),
    };
    let chosen = match chosen_apps(&project, args, &["--range", "--to"]) {
        Ok(a) => a,
        Err(e) => return fail(e),
    };
    let server = match server_of(&project, args) {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    let token = match token_for(&server) {
        Ok(t) => t,
        Err(e) => return fail(e),
    };
    let range = args.iter().position(|a| a == "--range").and_then(|i| args.get(i + 1)).cloned().unwrap_or_else(|| "24h".into());
    let fetch = |what: &str| -> Result<String, String> {
        match request("GET", &format!("{}/_platform/metrics?{}&range={}", server, what, range), &token, None) {
            Ok((200, b)) => b.split("\"summary\":").nth(1).and_then(|s| s.split('}').next()).map(str::to_string).ok_or_else(|| "the server's answer has no summary".to_string()),
            Ok((code, b)) => Err(format!("{} ({})", server_error(&b), code)),
            Err(e) => Err(e),
        }
    };
    let whole = chosen.len() == project.apps.len() && project.apps.len() > 1;
    let (title, query) = if whole || chosen.len() > 1 {
        (project.name.clone(), format!("project={}", project.name))
    } else {
        (project.server_name(chosen[0]), format!("app={}", project.server_name(chosen[0])))
    };
    let summary = match fetch(&query) {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    let n = |s: &str, k: &str| json_number(s, k).unwrap_or(0.0);
    let ms = |s: &str, k: &str| match json_number(s, k) {
        Some(v) if v >= 1000.0 => format!("{:.2} s", v / 1000.0),
        Some(v) if v >= 10.0 => format!("{:.0} ms", v),
        Some(v) => format!("{:.1} ms", v),
        None => "–".into(),
    };
    let summary = summary.as_str();
    let requests = n(summary, "requests");
    println!("{} on {}, last {}", title, server, range);
    if requests == 0.0 && n(summary, "events") == 0.0 {
        println!("  no requests");
        return ExitCode::SUCCESS;
    }
    println!("  requests      {:.0} ({:.2}/s)", requests, n(summary, "rps"));
    println!("  failed        {:.0} ({:.2}%: 5xx answers and timeouts)", n(summary, "errors"), 100.0 * n(summary, "errors") / requests.max(1.0));
    println!("  response time p50 {}  p95 {}  p99 {}", ms(summary, "p50_ms"), ms(summary, "p95_ms"), ms(summary, "p99_ms"));
    println!(
        "  on average    {} = cpu {} + database {} + outgoing http {} + other {}",
        ms(summary, "avg_ms"),
        ms(summary, "cpu_ms"),
        ms(summary, "db_ms"),
        ms(summary, "http_ms"),
        ms(summary, "other_ms")
    );
    println!("  per request   {:.1} database queries, {:.2} outgoing requests", n(summary, "db_queries") / requests.max(1.0), n(summary, "outbound_requests") / requests.max(1.0));
    if chosen.len() > 1 {
        println!("  by app        requests   failed      p95  avg");
        for app in &chosen {
            match fetch(&format!("app={}", project.server_name(app))) {
                Ok(s) => println!("    {:<12}{:>9.0}{:>9.0}{:>9}  {}", app.name, n(&s, "requests"), n(&s, "errors"), ms(&s, "p95_ms"), ms(&s, "avg_ms")),
                Err(e) => println!("    {:<12}{}", app.name, e),
            }
        }
    }
    let first = project.server_name(chosen[0]);
    println!("charts: {}/_platform/requests?app={}", server, first);
    ExitCode::SUCCESS
}
