// Projects and packages.
//
// A project is a directory with `plumb.toml`:
//
//     [package]
//     name = "notes"
//     version = "0.1.0"
//
//     [dependencies]
//     router = { git = "https://github.com/someone/router", version = "v1.2.0" }
//     redis = { git = "https://github.com/someone/tools", version = "v2.0.0", path = "packages/redis" }
//     mylib = { path = "../mylib" }
//
// `path` with `git` is a package in a subdirectory of the repository (one
// repository can hold several); `path` alone is a package on this disk
// (for developing packages side by side), relative to the project.
//
// `plumb add` records a dependency and pins the exact commit in `plumb.lock`.
// Packages are fetched with git into ~/.plumb/packages/<name>/<commit>/.
// There is no central registry: a package is a git repository and a
// version is a tag (like Go modules). One version of each package per build.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Default)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    pub deps: BTreeMap<String, Dep>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Dep {
    pub git: String,
    pub version: String,
    pub path: String,
}

#[derive(Debug, Clone, Default)]
pub struct Locked {
    pub git: String,
    pub version: String,
    pub commit: String,
}

// ---- a small TOML subset: [tables], key = "string", key = ["a", "b"],
// key = { k = "v", ... }, key = true

fn parse_toml(text: &str) -> Result<BTreeMap<String, BTreeMap<String, TomlValue>>, String> {
    let mut out: BTreeMap<String, BTreeMap<String, TomlValue>> = BTreeMap::new();
    let mut table = String::new();
    for (n, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            table = line.trim_matches(|c| c == '[' || c == ']').trim().to_string();
            out.entry(table.clone()).or_default();
            continue;
        }
        let (k, v) = line.split_once('=').ok_or(format!("line {}: expected `key = value`", n + 1))?;
        let value = parse_value(v.trim()).ok_or(format!("line {}: can't read the value", n + 1))?;
        out.entry(table.clone()).or_default().insert(k.trim().trim_matches('"').to_string(), value);
    }
    Ok(out)
}

#[derive(Debug, Clone)]
enum TomlValue {
    Str(String),
    List(Vec<String>),
    Table(BTreeMap<String, String>),
    Bool(bool),
}

fn parse_value(v: &str) -> Option<TomlValue> {
    match v {
        "true" => return Some(TomlValue::Bool(true)),
        "false" => return Some(TomlValue::Bool(false)),
        _ => {}
    }
    if let Some(s) = v.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
        return Some(TomlValue::Str(s.to_string()));
    }
    if let Some(inner) = v.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        let mut items = vec![];
        for part in inner.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            items.push(part.strip_prefix('"')?.strip_suffix('"')?.to_string());
        }
        return Some(TomlValue::List(items));
    }
    if let Some(inner) = v.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
        let mut m = BTreeMap::new();
        for part in inner.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let (k, v) = part.split_once('=')?;
            let v = v.trim().strip_prefix('"')?.strip_suffix('"')?;
            m.insert(k.trim().to_string(), v.to_string());
        }
        return Some(TomlValue::Table(m));
    }
    None
}

pub fn read_manifest(root: &Path) -> Result<Option<Manifest>, String> {
    let path = root.join("plumb.toml");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return Ok(None),
    };
    let t = parse_toml(&text).map_err(|e| format!("{}: {}", path.display(), e))?;
    let mut m = Manifest::default();
    if let Some(p) = t.get("package") {
        if let Some(TomlValue::Str(s)) = p.get("name") {
            m.name = s.clone();
        }
        if let Some(TomlValue::Str(s)) = p.get("version") {
            m.version = s.clone();
        }
    }
    if let Some(d) = t.get("dependencies") {
        for (name, v) in d {
            match v {
                TomlValue::Table(tbl) => {
                    let sub = tbl.get("path").cloned().unwrap_or_default();
                    let git = match tbl.get("git").cloned() {
                        Some(g) => g,
                        None if !sub.is_empty() => String::new(),
                        None => return Err(format!("{}: dependency `{}` needs `git = \"...\"` (or `path = \"...\"` for a package on this disk)", path.display(), name)),
                    };
                    let version = tbl.get("version").cloned().unwrap_or_default();
                    m.deps.insert(name.clone(), Dep { git, version, path: sub });
                }
                _ => return Err(format!("{}: write `{} = {{ git = \"...\", version = \"...\" }}`", path.display(), name)),
            }
        }
    }
    Ok(Some(m))
}

pub fn write_manifest(root: &Path, m: &Manifest) -> Result<(), String> {
    let mut s = format!("[package]\nname = \"{}\"\nversion = \"{}\"\n", m.name, m.version);
    if !m.deps.is_empty() {
        s += "\n[dependencies]\n";
        for (n, d) in &m.deps {
            let mut parts = vec![];
            if !d.git.is_empty() {
                parts.push(format!("git = \"{}\"", d.git));
            }
            if !d.version.is_empty() {
                parts.push(format!("version = \"{}\"", d.version));
            }
            if !d.path.is_empty() {
                parts.push(format!("path = \"{}\"", d.path));
            }
            s += &format!("{} = {{ {} }}\n", n, parts.join(", "));
        }
    }
    // the file's other tables ([app], [deploy]...) stay as they were
    if let Ok(old) = std::fs::read_to_string(root.join("plumb.toml")) {
        let mut keep = false;
        let mut rest = String::new();
        for line in old.lines() {
            let t = line.trim();
            if t.starts_with('[') {
                let name = t.trim_matches(|c| c == '[' || c == ']').trim();
                keep = name != "package" && name != "dependencies";
            }
            if keep {
                rest += line;
                rest.push('\n');
            }
        }
        if !rest.trim().is_empty() {
            s += "\n";
            s += rest.trim_end();
            s.push('\n');
        }
    }
    std::fs::write(root.join("plumb.toml"), s).map_err(|e| e.to_string())
}

/// What `plumb deploy` and `plumb dev` run, and where: a project of one or
/// more apps sharing domains, databases and secrets.
///
///     [project]
///     name = "shop"
///     domains = ["shop.example.com"]
///     databases = ["main"]
///     schema = "schema.sql"           # for the first database
///     outbound = ["api.stripe.com"]   # left out: any public host
///
///     [project.schema]                # or one per database
///     main = "schema.sql"
///
///     migrations = "migrations"       # steps run once per server, in order
///                                     # (the default, when that directory exists)
///
///     [apps.web]
///     main = "web/main.plumb"         # path left out: all of the domains
///
///     [apps.api]
///     main = "api/main.plumb"
///     path = "/api"                   # /api and below; the longest path wins
///     outbound = ["api.stripe.com"]   # this app's own list
///
///     [apps.site]                     # a frontend: files, served as they are
///     static = "frontend/dist"
///     build = "cd frontend && npm run build"   # run before a deploy
///     spa = true                      # page paths get index.html
///
///     [deploy]
///     server = "https://my-server.example.com"
///
/// A project of one app can be written as `[app]` with all of these keys
/// (and `main`, `websocket`) in it.
#[derive(Debug, Clone, Default)]
pub struct ProjectConfig {
    pub name: String,
    pub domains: Vec<String>,
    pub databases: Vec<String>,
    pub outbound: Option<Vec<String>>,
    /// database → schema file, relative to the project
    pub schema: BTreeMap<String, String>,
    /// The directory of migrations (`VERSION_NAME.plumb`, with `fn before()`
    /// and/or `fn after()`), relative to the project.
    pub migrations: Option<String>,
    pub server: Option<String>,
    /// by name
    pub apps: Vec<AppConfig>,
}

#[derive(Debug, Clone, Default)]
pub struct AppConfig {
    pub name: String,
    /// The program; none for an app of static files only.
    pub main: Option<String>,
    /// A directory of static files, served before the program (or alone).
    pub static_dir: Option<String>,
    /// A command that makes `static_dir` (`npm run build`), run in the
    /// project's directory.
    pub build: Option<String>,
    pub spa: bool,
    pub path: Option<String>,
    pub websocket: Option<String>,
    pub outbound: Option<Vec<String>>,
}

impl ProjectConfig {
    /// The server's name for one of the apps: the project's for the app
    /// named like it, `project.app` for the others.
    pub fn server_name(&self, app: &AppConfig) -> String {
        if app.name == self.name {
            self.name.clone()
        } else {
            format!("{}.{}", self.name, app.name)
        }
    }
}

fn valid_name(name: &str) -> bool {
    let b = name.as_bytes();
    !b.is_empty() && b.len() <= 63 && b[0].is_ascii_lowercase() && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

pub fn read_project(root: &Path) -> Result<ProjectConfig, String> {
    let path = root.join("plumb.toml");
    let text = std::fs::read_to_string(&path).map_err(|_| format!("there is no plumb.toml in {}; make the directory a project with `plumb init`", root.display()))?;
    let t = parse_toml(&text).map_err(|e| format!("{}: {}", path.display(), e))?;
    let file = path.display();
    let (section, single) = match (t.get("project"), t.get("app")) {
        (Some(p), None) => (p, false),
        (None, Some(a)) => (a, true),
        (Some(_), Some(_)) => return Err(format!("{}: [app] is for a project of one app; with [project], write it as [apps.NAME]", file)),
        (None, None) => {
            return Err(format!(
                "{}: there is no [app] section; add one:\n\n[app]\nname = \"myapp\"\nmain = \"main.plumb\"\ndatabases = [\"main\"]\nschema = \"schema.sql\"",
                file
            ))
        }
    };
    let table = if single { "app" } else { "project" };
    let s = |tbl: &BTreeMap<String, TomlValue>, at: &str, k: &str| match tbl.get(k) {
        Some(TomlValue::Str(v)) => Ok(Some(v.clone())),
        None => Ok(None),
        Some(_) => Err(format!("{}: [{}] {} must be text: {} = \"...\"", file, at, k, k)),
    };
    let list = |tbl: &BTreeMap<String, TomlValue>, at: &str, k: &str| match tbl.get(k) {
        Some(TomlValue::List(v)) => Ok(Some(v.clone())),
        None => Ok(None),
        Some(_) => Err(format!("{}: [{}] {} must be a list: {} = [\"...\"]", file, at, k, k)),
    };
    let allowed: &[&str] = if single {
        &["name", "main", "domains", "databases", "schema", "migrations", "outbound", "websocket", "static", "build", "spa"]
    } else {
        &["name", "domains", "databases", "schema", "migrations", "outbound"]
    };
    if let Some(k) = section.keys().find(|k| !allowed.contains(&k.as_str())) {
        let hint = if single { "" } else { " (an app's settings go in [apps.NAME])" };
        return Err(format!("{}: [{}] has no `{}`{}", file, table, k, hint));
    }
    let flag = |tbl: &BTreeMap<String, TomlValue>, at: &str, k: &str| match tbl.get(k) {
        Some(TomlValue::Bool(v)) => Ok(*v),
        None => Ok(false),
        Some(_) => Err(format!("{}: [{}] {} is true or false", file, at, k)),
    };
    // what an app is made of: a program, static files, or both
    let parts = |tbl: &BTreeMap<String, TomlValue>, at: &str, default_main: Option<&str>| -> Result<(Option<String>, Option<String>, Option<String>, bool), String> {
        let static_dir = s(tbl, at, "static")?;
        let main = s(tbl, at, "main")?.or_else(|| if static_dir.is_none() { default_main.map(str::to_string) } else { None });
        let build = s(tbl, at, "build")?;
        let spa = flag(tbl, at, "spa")?;
        if main.is_none() && static_dir.is_none() {
            return Err(format!("{}: [{}] needs main = \"main.plumb\" (a program), static = \"dist\" (files), or both", file, at));
        }
        if build.is_some() && static_dir.is_none() {
            return Err(format!("{}: [{}] build makes the static files: add static = \"the directory it makes\"", file, at));
        }
        if spa && (main.is_some() || static_dir.is_none()) {
            return Err(format!("{}: [{}] spa is for an app of static files only (with a program, the program answers what isn't a file)", file, at));
        }
        Ok((main, static_dir, build, spa))
    };
    let name = s(section, table, "name")?.ok_or_else(|| format!("{}: [{}] needs a name: name = \"myapp\"", file, table))?;
    if !valid_name(&name) {
        return Err(format!("{}: [{}] name {:?}: lower-case letters, digits and -, starting with a letter", file, table, name));
    }
    let mut c = ProjectConfig {
        name: name.clone(),
        domains: list(section, table, "domains")?.unwrap_or_default(),
        databases: list(section, table, "databases")?.unwrap_or_default(),
        outbound: list(section, table, "outbound")?,
        ..Default::default()
    };
    if single {
        let (main, static_dir, build, spa) = parts(section, table, Some("main.plumb"))?;
        c.apps.push(AppConfig { name, main, static_dir, build, spa, websocket: s(section, table, "websocket")?, ..Default::default() });
    } else {
        for (tname, tbl) in &t {
            let Some(app) = tname.strip_prefix("apps.") else { continue };
            let at = tname.as_str();
            if !valid_name(app) {
                return Err(format!("{}: [{}]: an app's name is lower-case letters, digits and -, starting with a letter", file, at));
            }
            if let Some(k) = tbl.keys().find(|k| !["main", "path", "websocket", "outbound", "static", "build", "spa"].contains(&k.as_str())) {
                let hint = if ["domains", "databases", "schema"].contains(&k.as_str()) { " (the project's apps share it: put it in [project])" } else { "" };
                return Err(format!("{}: [{}] has no `{}`{}", file, at, k, hint));
            }
            let path = s(tbl, at, "path")?;
            if let Some(p) = &path {
                let ok = p == "/" || (p.starts_with('/') && !p.ends_with('/') && !p.contains("//") && !p.contains(['?', '#', ' ']));
                if !ok {
                    return Err(format!("{}: [{}] path {:?} must start with / and not end with one: path = \"/api\"", file, at, p));
                }
                if c.apps.iter().any(|a| a.path.as_deref().unwrap_or("/") == p) {
                    return Err(format!("{}: two apps have path {:?}", file, p));
                }
            }
            let (main, static_dir, build, spa) = parts(tbl, at, None)?;
            c.apps.push(AppConfig { name: app.to_string(), main, static_dir, build, spa, path, websocket: s(tbl, at, "websocket")?, outbound: list(tbl, at, "outbound")? });
        }
        if c.apps.is_empty() {
            return Err(format!("{}: the project has no apps; add one:\n\n[apps.web]\nmain = \"main.plumb\"", file));
        }
        if c.apps.iter().filter(|a| a.path.as_deref().unwrap_or("/") == "/").count() > 1 {
            return Err(format!("{}: more than one app serves the whole domain: give all but one a path = \"/...\"", file));
        }
    }
    if let Some(f) = s(section, table, "schema")? {
        let first = c.databases.first().ok_or_else(|| format!("{}: [{}] schema is for the first database, and there are none: databases = [\"main\"]", file, table))?;
        c.schema.insert(first.clone(), f);
    }
    if let Some(tbl) = t.get(&format!("{}.schema", table)) {
        for (db, v) in tbl {
            match v {
                TomlValue::Str(f) if c.databases.contains(db) => {
                    c.schema.insert(db.clone(), f.clone());
                }
                TomlValue::Str(_) => return Err(format!("{}: [{}.schema] {}: there is no database {:?} in databases", file, table, db, db)),
                _ => return Err(format!("{}: [{}.schema] {} = \"file.sql\"", file, table, db)),
            }
        }
    }
    c.migrations = s(section, table, "migrations")?;
    if c.migrations.is_none() && root.join("migrations").is_dir() {
        c.migrations = Some("migrations".to_string());
    }
    if let Some(TomlValue::Str(server)) = t.get("deploy").and_then(|d| d.get("server")) {
        c.server = Some(server.trim_end_matches('/').to_string());
    }
    Ok(c)
}

pub fn read_lock(root: &Path) -> BTreeMap<String, Locked> {
    let mut out = BTreeMap::new();
    let text = match std::fs::read_to_string(root.join("plumb.lock")) {
        Ok(t) => t,
        Err(_) => return out,
    };
    if let Ok(t) = parse_toml(&text) {
        for (name, tbl) in t {
            if let Some(n) = name.strip_prefix("package.") {
                let get = |k: &str| match tbl.get(k) {
                    Some(TomlValue::Str(s)) => s.clone(),
                    _ => String::new(),
                };
                out.insert(n.to_string(), Locked { git: get("git"), version: get("version"), commit: get("commit") });
            }
        }
    }
    out
}

pub fn write_lock(root: &Path, lock: &BTreeMap<String, Locked>) -> Result<(), String> {
    let mut s = String::from("# Written by `plumb add` / `plumb fetch`. Pins every package to an exact commit.\n");
    for (n, l) in lock {
        s += &format!("\n[package.{}]\ngit = \"{}\"\nversion = \"{}\"\ncommit = \"{}\"\n", n, l.git, l.version, l.commit);
    }
    std::fs::write(root.join("plumb.lock"), s).map_err(|e| e.to_string())
}

// The project root: the nearest directory with `plumb.toml`, else the file's directory.
pub fn find_root(file: &Path) -> PathBuf {
    let abs = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let mut dir = abs.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."));
    let start = dir.clone();
    loop {
        if dir.join("plumb.toml").exists() {
            return dir;
        }
        if !dir.pop() {
            return start;
        }
    }
}

pub fn packages_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".plumb").join("packages")
}

pub fn package_path(name: &str, commit: &str) -> PathBuf {
    packages_dir().join(name).join(commit)
}

fn git(args: &[&str], dir: Option<&Path>) -> Result<String, String> {
    let mut c = Command::new("git");
    c.args(args);
    if let Some(d) = dir {
        c.current_dir(d);
    }
    let out = c.output().map_err(|e| format!("can't run git: {}", e))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

// Downloads `dep` (the tag `version`, or the default branch) and returns the
// exact commit. The files end up in package_path(name, commit).
pub fn fetch(name: &str, dep: &Dep) -> Result<Locked, String> {
    let tmp = packages_dir().join(format!(".download-{}-{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(packages_dir()).map_err(|e| e.to_string())?;
    let mut args = vec!["clone", "--quiet", "--depth", "1"];
    if !dep.version.is_empty() {
        args.push("--branch");
        args.push(&dep.version);
    }
    let tmp_s = tmp.to_string_lossy().to_string();
    args.push(&dep.git);
    args.push(&tmp_s);
    git(&args, None).map_err(|e| format!("can't download `{}` from {}: {}", name, dep.git, e))?;
    let commit = git(&["rev-parse", "HEAD"], Some(&tmp))?;
    let dest = package_path(name, &commit);
    if dest.exists() {
        let _ = std::fs::remove_dir_all(&tmp);
    } else {
        std::fs::create_dir_all(dest.parent().unwrap()).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_dir_all(tmp.join(".git"));
        std::fs::rename(&tmp, &dest).map_err(|e| e.to_string())?;
    }
    Ok(Locked { git: dep.git.clone(), version: dep.version.clone(), commit })
}

// Makes sure every dependency (and theirs) is downloaded and pinned.
// Returns package name -> directory.
pub fn resolve(root: &Path, update: bool) -> Result<BTreeMap<String, PathBuf>, String> {
    let manifest = match read_manifest(root)? {
        Some(m) => m,
        None => return Ok(BTreeMap::new()),
    };
    let mut lock = read_lock(root);
    let mut dirs = BTreeMap::new();
    let mut pending: Vec<(String, Dep, String)> = manifest.deps.iter().map(|(n, d)| (n.clone(), d.clone(), manifest.name.clone())).collect();
    let mut changed = false;
    while let Some((name, dep, wanted_by)) = pending.pop() {
        if dep.git.is_empty() {
            // on this disk: nothing to download or pin
            if dirs.contains_key(&name) {
                continue;
            }
            let dir = root.join(&dep.path);
            if !dir.join(format!("{}.plumb", name)).exists() {
                return Err(format!("package `{}`: there is no {}.plumb in {}", name, name, dir.display()));
            }
            if let Some(sub) = read_manifest(&dir)? {
                for (n, d) in sub.deps {
                    let d = if d.git.is_empty() { Dep { path: dir.join(&d.path).to_string_lossy().to_string(), ..d } } else { d };
                    pending.push((n, d, name.clone()));
                }
            }
            dirs.insert(name, dir);
            continue;
        }
        if dirs.contains_key(&name) {
            if let Some(l) = lock.get(&name) {
                if l.git != dep.git {
                    return Err(format!("`{}` is wanted from two places: {} and {} (by `{}`)", name, l.git, dep.git, wanted_by));
                }
            }
            continue;
        }
        let locked = match lock.get(&name) {
            Some(l) if !update && l.git == dep.git && l.version == dep.version && package_path(&name, &l.commit).exists() => l.clone(),
            Some(l) if !update && l.git == dep.git && l.version == dep.version => {
                // pinned but not downloaded here yet: fetch exactly that commit
                let got = fetch(&name, &dep)?;
                if got.commit != l.commit {
                    return Err(format!("`{}` {} now points to commit {}, but plumb.lock pins {}; run `plumb update` if the change is expected", name, dep.version, got.commit, l.commit));
                }
                got
            }
            _ => {
                changed = true;
                fetch(&name, &dep)?
            }
        };
        let dir = package_path(&name, &locked.commit).join(&dep.path);
        if !dir.join(format!("{}.plumb", name)).exists() {
            return Err(format!("package `{}`: there is no {}.plumb in {}{}", name, name, dep.git, if dep.path.is_empty() { String::new() } else { format!(" / {}", dep.path) }));
        }
        if let Some(sub) = read_manifest(&dir)? {
            for (n, d) in sub.deps {
                pending.push((n, d, name.clone()));
            }
        }
        lock.insert(name.clone(), locked);
        dirs.insert(name, dir);
    }
    lock.retain(|n, _| dirs.contains_key(n));
    if changed || update {
        write_lock(root, &lock)?;
    }
    Ok(dirs)
}

#[cfg(test)]
mod tests {
    use super::read_project;

    fn read(toml: &str) -> Result<super::ProjectConfig, String> {
        let dir = std::env::temp_dir().join(format!("plumb-project-test-{}-{}", std::process::id(), toml.len()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("plumb.toml"), toml).unwrap();
        let r = read_project(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        r
    }

    #[test]
    fn projects() {
        let p = read("[project]\nname = \"shop\"\ndatabases = [\"main\"]\nschema = \"s.sql\"\n\n[apps.web]\nmain = \"web.plumb\"\n\n[apps.api]\nmain = \"api.plumb\"\npath = \"/api\"\n").unwrap();
        assert_eq!(p.apps.iter().map(|a| p.server_name(a)).collect::<Vec<_>>(), ["shop.api", "shop.web"]);
        assert_eq!(p.schema["main"], "s.sql");
        let one = read("[app]\nname = \"notes\"\ndomains = [\"n.example.com\"]\n").unwrap();
        assert_eq!((one.apps.len(), one.server_name(&one.apps[0]), one.apps[0].main.as_deref()), (1, "notes".to_string(), Some("main.plumb")));
        assert!(read("[project]\nname = \"shop\"\n").unwrap_err().contains("no apps"));
        assert!(read("[project]\nname = \"shop\"\n[apps.a]\nmain = \"a.plumb\"\n[apps.b]\nmain = \"b.plumb\"\n").unwrap_err().contains("whole domain"));
        assert!(read("[project]\nname = \"shop\"\n[apps.a]\nmain = \"a.plumb\"\ndatabases = [\"x\"]\n").unwrap_err().contains("put it in [project]"));
        assert!(read("[project]\nname = \"shop\"\n[apps.a]\nmain = \"a.plumb\"\npath = \"/api/\"\n").unwrap_err().contains("must start with /"));
        let site = read("[project]\nname = \"shop\"\n[apps.site]\nstatic = \"dist\"\nspa = true\n[apps.api]\nmain = \"a.plumb\"\npath = \"/api\"\n").unwrap();
        let s = site.apps.iter().find(|a| a.name == "site").unwrap();
        assert!(s.main.is_none() && s.spa && s.static_dir.as_deref() == Some("dist"));
        assert!(read("[project]\nname = \"shop\"\n[apps.a]\npath = \"/a\"\n").unwrap_err().contains("needs main"));
        assert!(read("[project]\nname = \"shop\"\n[apps.a]\nmain = \"a.plumb\"\nstatic = \"d\"\nspa = true\n").unwrap_err().contains("static files only"));
    }
}
