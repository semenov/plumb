// `plumb api`, `plumb sql`, `plumb export`: a project described and taken
// away in plain formats, so it can run elsewhere or be rewritten (by a
// person or an AI) in another language: the code, its API as Markdown and
// OpenAPI, its SQL with what is SQLite-only, its databases (SQLite files
// and SQL), its users' files, and the names of its secrets.

use crate::api;
use crate::deploy::{self, json_str, json_strings};
use crate::project::ProjectConfig;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

fn fail(msg: String) -> ExitCode {
    eprintln!("error: {}", msg);
    ExitCode::from(1)
}

/// Each app with a program: (name, its path on the domains, routes, SQL).
type Described = (String, String, Vec<api::Route>, Vec<(String, String)>, types_holder::Holder);

mod types_holder {
    /// A checked program kept alive for writing its types out.
    pub struct Holder(pub crate::types::Program);
}

fn describe_apps(root: &Path, p: &ProjectConfig) -> Result<Vec<Described>, String> {
    let mut out = vec![];
    for app in &p.apps {
        let Some(main) = &app.main else { continue };
        let path = root.join(main);
        let mut sources = crate::diag::Sources::default();
        let (prog, _) = crate::front_end(&path.to_string_lossy(), &mut sources).ok_or_else(|| format!("{} doesn't compile (the errors are above)", main))?;
        let own: BTreeSet<usize> = api::own_modules(&prog, crate::STD_NAMES);
        let is_own = |m: usize| own.contains(&m);
        let routes = api::routes(&prog, &sources, &is_own);
        let sql = api::sql_literals(&prog, &sources, &is_own);
        out.push((app.name.clone(), app.path.clone().unwrap_or_else(|| "/".into()), routes, sql, types_holder::Holder(prog)));
    }
    Ok(out)
}

fn api_markdown(described: &[Described]) -> String {
    // each app is a program of its own, with its own types
    let mut s = String::new();
    for (i, (name, prefix, routes, _, holder)) in described.iter().enumerate() {
        let md = api::markdown(&holder.0, &[(name.clone(), prefix.clone(), routes.clone())]);
        if i == 0 {
            s += &md;
        } else {
            // without the title and introduction again
            s += md.splitn(2, "\n## ").nth(1).map(|rest| format!("\n## {}", rest)).as_deref().unwrap_or("");
        }
    }
    if described.is_empty() {
        s += "# API\n\nThe project has no programs (only static files).\n";
    }
    s
}

fn openapi(p: &ProjectConfig, described: &[Described]) -> String {
    let docs: Vec<String> = described.iter().map(|(name, prefix, routes, _, holder)| api::openapi(&holder.0, &p.name, &[(name.clone(), prefix.clone(), routes.clone())])).collect();
    match docs.len() {
        0 => format!("{{\"openapi\":\"3.0.3\",\"info\":{{\"title\":{},\"version\":\"1\"}},\"paths\":{{}}}}\n", json_str(&p.name)),
        1 => docs[0].clone(),
        _ => merge_openapi(&docs),
    }
}

/// Several OpenAPI documents (one per app) as one: their `paths` and
/// `schemas` side by side.
fn merge_openapi(docs: &[String]) -> String {
    let part = |d: &str, start: &str, end: &str| -> String {
        let a = d.find(start).map(|i| i + start.len()).unwrap_or(0);
        let b = d.rfind(end).unwrap_or(d.len());
        d.get(a..b).unwrap_or("").to_string()
    };
    let paths: Vec<String> = docs.iter().map(|d| part(d, "\"paths\":{", "},\"components\"")).filter(|p| !p.is_empty()).collect();
    let schemas: Vec<String> = docs.iter().map(|d| part(d, "\"schemas\":{", "}}}")).filter(|p| !p.is_empty()).collect();
    let head = docs.first().map(|d| d[..d.find("\"paths\"").unwrap_or(0)].to_string()).unwrap_or_default();
    format!("{}\"paths\":{{{}}},\"components\":{{\"schemas\":{{{}}}}}}}\n", head, paths.join(","), schemas.join(","))
}

fn sql_markdown(described: &[Described], migrations: &[(String, String)]) -> String {
    let mut s = String::from("# SQL\n\nEvery SQL statement in the project's code, where it is, and what in it works only in SQLite (with what to use in Postgres and most other databases). Values are parameters: `?` (or `$1`, `$2`).\n");
    let mut seen = BTreeSet::new();
    for (app, _, _, sql, _) in described {
        s += &format!("\n## App `{}`\n", app);
        for (at, q) in sql {
            if !seen.insert((at.clone(), q.clone())) {
                continue;
            }
            s += &format!("\n{}\n```sql\n{}\n```\n", at, q.trim());
            for note in api::portability(q) {
                s += &format!("- SQLite only: {}\n", note);
            }
        }
    }
    if !migrations.is_empty() {
        s += "\n## Migrations\n";
        for (at, q) in migrations {
            s += &format!("\n{}\n```sql\n{}\n```\n", at, q.trim());
            for note in api::portability(q) {
                s += &format!("- SQLite only: {}\n", note);
            }
        }
    }
    s
}

/// The SQL in the project's migrations (each file checked on its own).
fn migration_sql(root: &Path, p: &ProjectConfig) -> Vec<(String, String)> {
    let mut out = vec![];
    let Some(dir) = &p.migrations else { return out };
    let Ok(entries) = std::fs::read_dir(root.join(dir)) else { return out };
    let mut files: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "plumb")).collect();
    files.sort();
    for f in files {
        let mut sources = crate::diag::Sources::default();
        if let Ok((c, _)) = crate::analyze(&f.to_string_lossy(), &mut sources, &std::collections::HashMap::new(), false) {
            let own = api::own_modules(&c.prog, crate::STD_NAMES);
            out.extend(api::sql_literals(&c.prog, &sources, &|m| own.contains(&m)));
        }
    }
    out
}

fn project() -> Result<(PathBuf, ProjectConfig), String> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let root = crate::project::find_root(&cwd.join("x.plumb"));
    let p = crate::project::read_project(&root)?;
    Ok((root, p))
}

/// `plumb api`: the project's API, read from its code, as Markdown
/// (`--openapi`: as OpenAPI).
pub fn api_cmd(args: &[String]) -> ExitCode {
    let (root, p) = match project() {
        Ok(x) => x,
        Err(e) => return fail(e),
    };
    let described = match describe_apps(&root, &p) {
        Ok(d) => d,
        Err(e) => return fail(e),
    };
    if args.iter().any(|a| a == "--openapi") {
        print!("{}", openapi(&p, &described));
    } else {
        print!("{}", api_markdown(&described));
    }
    ExitCode::SUCCESS
}

/// `plumb sql`: every SQL statement of the project, and what in it is
/// SQLite-only.
pub fn sql_cmd(_args: &[String]) -> ExitCode {
    let (root, p) = match project() {
        Ok(x) => x,
        Err(e) => return fail(e),
    };
    let described = match describe_apps(&root, &p) {
        Ok(d) => d,
        Err(e) => return fail(e),
    };
    print!("{}", sql_markdown(&described, &migration_sql(&root, &p)));
    ExitCode::SUCCESS
}

/// The project's own files: git's list when it's a repository, else every
/// file but the generated and the data.
fn source_files(root: &Path) -> Vec<PathBuf> {
    let git = Command::new("git").args(["ls-files", "-co", "--exclude-standard"]).current_dir(root).output();
    if let Some(o) = git.ok().filter(|o| o.status.success()) {
        return String::from_utf8_lossy(&o.stdout).lines().filter(|l| !l.is_empty() && !l.starts_with("export/")).map(PathBuf::from).collect();
    }
    fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || name.ends_with(".log") || ["node_modules", "data", "storage", "export", "target", "dist"].contains(&name.as_str()) {
                continue;
            }
            let p = e.path();
            if p.is_dir() {
                walk(root, &p, out);
            } else if let Ok(rel) = p.strip_prefix(root) {
                out.push(rel.to_path_buf());
            }
        }
    }
    let mut out = vec![];
    walk(root, root, &mut out);
    out
}

/// `plumb export [DIR] [--local]`: everything to run the project elsewhere
/// or rewrite it: code, API, SQL, data, files, secret names, and a README
/// saying how. The data comes from the project's server, or with `--local`
/// from this machine (data/, storage/).
pub fn export(args: &[String]) -> ExitCode {
    let (root, p) = match project() {
        Ok(x) => x,
        Err(e) => return fail(e),
    };
    let local = args.iter().any(|a| a == "--local");
    let date = Command::new("date").args(["-u", "+%Y%m%d-%H%M"]).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    let dir = args.iter().enumerate().find(|(i, a)| !a.starts_with('-') && (*i == 0 || args[i - 1] != "--to")).map(|(_, a)| PathBuf::from(a)).unwrap_or_else(|| root.join("export").join(format!("{}-{}", p.name, date)));
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return fail(format!("{}: {}", dir.display(), e));
    }
    let shown = |p: &Path| p.strip_prefix(&root).unwrap_or(p).display().to_string();
    eprintln!("exporting {} to {}", p.name, shown(&dir));
    // the code
    let src = dir.join("source");
    let mut n = 0;
    for f in source_files(&root) {
        let (from, to) = (root.join(&f), src.join(&f));
        if from.is_file() {
            if let Some(d) = to.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            if std::fs::copy(&from, &to).is_ok() {
                n += 1;
            }
        }
    }
    eprintln!("  source/: {} files", n);
    // what it does
    let described = match describe_apps(&root, &p) {
        Ok(d) => d,
        Err(e) => return fail(e),
    };
    let routes: usize = described.iter().map(|d| d.2.len()).sum();
    let write = |name: &str, text: String| -> bool {
        match std::fs::write(dir.join(name), text) {
            Ok(()) => true,
            Err(e) => {
                eprintln!("error: {}: {}", name, e);
                false
            }
        }
    };
    if !write("API.md", api_markdown(&described)) || !write("openapi.json", openapi(&p, &described)) || !write("SQL.md", sql_markdown(&described, &migration_sql(&root, &p))) {
        return ExitCode::from(1);
    }
    eprintln!("  API.md, openapi.json: {} routes; SQL.md", routes);
    // the data
    let data = dir.join("data");
    let _ = std::fs::create_dir_all(&data);
    let mut secrets: Vec<String> = vec![];
    let mut from = String::from("this machine");
    if local {
        for db in &p.databases {
            let f = root.join("data").join(format!("{}.db", db));
            if f.exists() {
                let _ = std::fs::copy(&f, data.join(format!("{}.db", db)));
                if let Ok(o) = Command::new("sqlite3").arg(&f).arg(".dump").output() {
                    if o.status.success() {
                        let _ = std::fs::write(data.join(format!("{}.sql", db)), o.stdout);
                    }
                }
            }
        }
        if root.join("storage").is_dir() {
            let _ = Command::new("cp").arg("-R").arg(root.join("storage")).arg(dir.join("storage")).status();
        }
        if let Ok(text) = std::fs::read_to_string(root.join(".env")) {
            secrets = text.lines().filter_map(|l| l.trim().strip_prefix("export ").unwrap_or(l.trim()).split_once('=').map(|(k, _)| k.trim().to_string())).filter(|k| !k.is_empty() && !k.starts_with('#')).collect();
        }
    } else {
        let (server, token) = match deploy::server_and_token(&p, args) {
            Ok(x) => x,
            Err(e) => return fail(format!("{} (or export this machine's data: plumb export --local)", e)),
        };
        from = server.clone();
        for db in &p.databases {
            let base = format!("{}/_platform/projects/{}/databases/{}", server, p.name, db);
            for (what, ext) in [("snapshot", "db"), ("dump", "sql")] {
                let out = data.join(format!("{}.{}", db, ext));
                match deploy::download(&format!("{}/{}", base, what), &token, &out) {
                    Ok(200) => {}
                    Ok(code) => {
                        let body = std::fs::read_to_string(&out).unwrap_or_default();
                        let _ = std::fs::remove_file(&out);
                        eprintln!("  database {}: {} ({})", db, deploy::server_error(&body), code);
                    }
                    Err(e) => return fail(e),
                }
            }
        }
        match deploy::pull_files(&p, &server, &token, &dir.join("storage")) {
            Ok((n, _)) => eprintln!("  storage/: {} files", n),
            Err(e) => return fail(e),
        }
        if let Ok((200, body)) = deploy::request("GET", &format!("{}/_platform/projects/{}/secrets", server, p.name), &token, None) {
            secrets = json_strings(&body, "names");
        }
    }
    eprintln!("  data/: {} from {}", p.databases.iter().map(|d| format!("{}.db, {}.sql", d, d)).collect::<Vec<_>>().join(", "), from);
    let mut sec = String::from("# Secrets\n\nThe environment variables the project's apps read (their values aren't exported: take them from where you keep them).\n\n");
    for s in &secrets {
        sec += &format!("- `{}`\n", s);
    }
    if secrets.is_empty() {
        sec += "(none)\n";
    }
    write("SECRETS.md", sec);
    write("README.md", readme(&p, &described, &from, &date));
    println!("{}", shown(&dir));
    ExitCode::SUCCESS
}

fn readme(p: &ProjectConfig, described: &[Described], from: &str, date: &str) -> String {
    let apps: Vec<String> = p
        .apps
        .iter()
        .map(|a| {
            let what = match (&a.main, &a.static_dir) {
                (Some(m), Some(s)) => format!("program `source/{}` with static files `source/{}`", m, s),
                (Some(m), None) => format!("program `source/{}`", m),
                (None, Some(s)) => format!(
                    "static files `source/{}`{}{}",
                    s,
                    a.build.as_ref().map(|b| format!(", built by `{}`", b)).unwrap_or_default(),
                    if a.spa { " (a single-page app: unknown paths get index.html)" } else { "" }
                ),
                (None, None) => String::new(),
            };
            let routes = described.iter().find(|d| d.0 == a.name).map_or(0, |d| d.2.len());
            format!("- `{}` at `{}`: {}{}", a.name, a.path.clone().unwrap_or_else(|| "/".into()), what, if routes > 0 { format!(", {} route{}", routes, if routes == 1 { "" } else { "s" }) } else { String::new() })
        })
        .collect();
    format!(
        r#"# {name}

Exported {date} (UTC), data from {from}. Everything here is in plain formats.

## What's here

- `source/`: the project's code (Plumb), `plumb.toml` and its migrations.
- `API.md`, `openapi.json`: every route of every app, with what it takes,
  what it answers and the SQL it runs, read from the code.
- `SQL.md`: every SQL statement and what in it works only in SQLite.
- `data/NAME.db`: each database as a SQLite file (`sqlite3 data/NAME.db`);
  `data/NAME.sql`: the same as SQL.
- `storage/`: the users' files: `storage/data/KEY` is a file's content,
  `storage/meta/KEY.json` its type and whether it's public.
- `SECRETS.md`: the names of the environment variables the apps read.

The project's apps (one domain: {domains}; each app takes the paths under its own):

{apps}

## Running it as it is, without the platform

Plumb programs also build as ordinary servers. Put `data/` and `storage/`
next to the code (`source/data/`, `source/storage/`) and, for each program:

    cd source && plumb build APP/main.plumb -o app && ./app

`db.open("main")` opens `data/main.db` there; secrets are environment
variables. Serve static files and route paths with any web server (nginx,
Caddy) as listed above.

## Rewriting it in another language

1. The contract is `API.md` (and `openapi.json` for tools): keep every
   path, method, status and JSON field name, so the website or app that
   calls it keeps working unchanged.
2. The data model is `data/NAME.sql` (the schema at its top), and
   `source/migrations/` shows how it got there.
3. The behaviour is in `source/`: each route names its handler and where it
   is (file:line), and lists the SQL it runs.
4. `SQL.md` marks the SQLite-only parts if you move to Postgres.
5. Check the new version against the old: load copies of `data/` into
   both, send the same requests (one per route in `openapi.json`) and
   compare the answers.
"#,
        name = p.name,
        date = date,
        from = from,
        domains = if p.domains.is_empty() { "none set".to_string() } else { p.domains.join(", ") },
        apps = apps.join("\n"),
    )
}
