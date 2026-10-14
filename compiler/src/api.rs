// What a program's HTTP API is, read from its checked code (`plumb export`,
// `plumb api`): every route registered on an `http.Router` with a written
// path, and for each handler, following the program's own functions, the
// types it decodes from the request, the responses it builds, and the SQL it
// runs. Written as Markdown (for a person or an AI rewriting the app) and
// as OpenAPI.

use crate::diag::Sources;
use crate::types::{Callee, Program, TBlock, TExpr, TK, TStmt, Ty, TypeKind};
use std::collections::{BTreeMap, BTreeSet, HashSet};

#[derive(Clone)]
pub struct Route {
    pub method: String,
    pub path: String,
    /// `file.plumb:12`
    pub at: String,
    pub handler: String,
    /// (how: json, form, query; the type)
    pub requests: Vec<(String, Ty)>,
    /// (status if written, the body: json of a type, or text, html, ...)
    pub responses: Vec<(Option<i64>, Response)>,
    pub sql: Vec<String>,
}

#[derive(Clone)]
pub enum Response {
    Json(Ty),
    Other(String),
}

/// Every SQL literal of the program's own code, with where it is.
pub fn sql_literals(prog: &Program, sources: &Sources, own: &dyn Fn(usize) -> bool) -> Vec<(String, String)> {
    let mut out = vec![];
    for f in prog.fns.iter().filter(|f| own(f.module)) {
        if let Some(b) = &f.body {
            walk_block(&b.block, &mut |e| {
                if let Some(sql) = sql_text(prog, e) {
                    out.push((at(sources, e), sql));
                }
            });
        }
    }
    let key = |at: &str| -> (String, usize) {
        let (f, l) = at.rsplit_once(':').unwrap_or((at, "0"));
        (f.to_string(), l.parse().unwrap_or(0))
    };
    out.sort_by(|a, b| key(&a.0).cmp(&key(&b.0)).then(a.1.cmp(&b.1)));
    out.dedup();
    out
}

/// An SQL statement of the program for the server to check against the
/// project's database before it goes live: where it is, and for
/// `query<T>` the fields of `T` (all, and the ones without a default).
pub struct SqlCheck {
    pub at: String,
    pub sql: String,
    pub fields: Option<(Vec<String>, Vec<String>)>,
}

pub fn sql_checks(prog: &Program, sources: &Sources, own: &dyn Fn(usize) -> bool) -> Vec<SqlCheck> {
    let mut out: Vec<SqlCheck> = vec![];
    let mut seen = HashSet::new();
    for f in prog.fns.iter().filter(|f| own(f.module)) {
        let Some(b) = &f.body else { continue };
        walk_block(&b.block, &mut |e| {
            // `conn.query<T>(sql, ...)`: the columns must fill T
            if let TK::Call { callee: Callee::Fn(id, targs), args, .. } = &e.kind {
                let callee = &prog.fns[*id];
                let on_conn = callee.name == "query" && callee.owner.is_some_and(|o| prog.defs[o].name == "Connection" && prog.module_names.get(prog.defs[o].module).map(String::as_str) == Some("db"));
                if let (true, Some(sql_arg), Some(t)) = (on_conn, args.get(1), targs.first()) {
                    if let Some(sql) = sql_text(prog, sql_arg) {
                        if seen.insert((sql_arg.span.file, sql_arg.span.lo)) {
                            out.push(SqlCheck { at: at(sources, sql_arg), sql, fields: record_fields(prog, t) });
                        }
                    }
                }
            }
            if let Some(sql) = sql_text(prog, e) {
                if seen.insert((e.span.file, e.span.lo)) {
                    out.push(SqlCheck { at: at(sources, e), sql, fields: None });
                }
            }
        });
    }
    out
}

/// A record's field names: all, and those it can't do without.
fn record_fields(prog: &Program, t: &Ty) -> Option<(Vec<String>, Vec<String>)> {
    let Ty::Adt(d, _) = t else { return None };
    let TypeKind::Record { fields } = &prog.defs[*d].kind else { return None };
    let all = fields.iter().map(|f| f.name.clone()).collect();
    let required = fields.iter().filter(|f| f.default.is_none() && !matches!(f.ty, Ty::Opt(_))).map(|f| f.name.clone()).collect();
    Some((all, required))
}

fn at(sources: &Sources, e: &TExpr) -> String {
    let file = sources.files.get(e.span.file as usize).map(|f| f.path.clone()).unwrap_or_default();
    let name = std::path::Path::new(&file).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or(file);
    format!("{}:{}", name, sources.line_col(e.span).0)
}

fn sql_text(prog: &Program, e: &TExpr) -> Option<String> {
    if let (TK::Wrap(inner), Ty::Adt(d, _)) = (&e.kind, &e.ty) {
        if let TK::Text(s) = &inner.kind {
            let def = &prog.defs[*d];
            if def.name == "Query" && prog.module_names.get(def.module).map(String::as_str) == Some("sql") {
                return Some(s.clone());
            }
        }
    }
    None
}

/// The routes of a program. `own`: whether a module is the program's (not
/// the standard library).
pub fn routes(prog: &Program, sources: &Sources, own: &dyn Fn(usize) -> bool) -> Vec<Route> {
    let mut out = vec![];
    for f in prog.fns.iter().filter(|f| own(f.module)) {
        let Some(b) = &f.body else { continue };
        walk_block(&b.block, &mut |e| {
            // `router.get(...)`: a call with the router first, or (on a
            // `var`) a mutating call without it
            let (id, args): (&usize, &[TExpr]) = match &e.kind {
                TK::Call { callee: Callee::Fn(id, _), args, .. } if !args.is_empty() => (id, &args[1..]),
                TK::MutCall { fn_id, args, .. } => (fn_id, &args[..]),
                _ => return,
            };
            let callee = &prog.fns[*id];
            let on_router = callee.owner.is_some_and(|o| prog.defs[o].name == "Router" && prog.module_names.get(prog.defs[o].module).map(String::as_str) == Some("http"));
            if !on_router {
                return;
            }
            let text = |i: usize| match args.get(i).map(|a| &a.kind) {
                Some(TK::Text(s)) => Some(s.clone()),
                _ => None,
            };
            let (method, path, handler) = match callee.name.as_str() {
                m @ ("get" | "post" | "put" | "patch" | "delete" | "any") => (m.to_uppercase(), text(0), args.get(1)),
                "add" => (text(0).unwrap_or_default().to_uppercase(), text(1), args.get(2)),
                "websocket" => ("WEBSOCKET".to_string(), text(0), args.get(1)),
                "files" => ("GET".to_string(), text(0).map(|p| format!("{}/*", p.trim_end_matches('/'))), None),
                _ => return,
            };
            let Some(path) = path else { return };
            let mut r = Route { method, path, at: at(sources, e), handler: String::new(), requests: vec![], responses: vec![], sql: vec![] };
            match handler {
                Some(h) => {
                    r.handler = match &h.kind {
                        TK::FnRef(fid, _) => prog.fns[*fid].name.clone(),
                        // `req => try save(req, conn)`: the function it hands over to
                        TK::Lambda { body, .. } => match &body.kind {
                            TK::Try { call, .. } => called_name(prog, call, own),
                            _ => called_name(prog, body, own),
                        }
                        .unwrap_or_else(|| "inline".into()),
                        _ => "a function value".into(),
                    };
                    let mut seen = HashSet::new();
                    describe(prog, h, own, &mut seen, &mut r, 0);
                }
                None => {
                    r.handler = "static files".into();
                    r.responses.push((Some(200), Response::Other("a file".into())));
                }
            }
            out.push(r);
        });
    }
    out
}

fn called_name(prog: &Program, e: &TExpr, own: &dyn Fn(usize) -> bool) -> Option<String> {
    match &e.kind {
        TK::Call { callee: Callee::Fn(id, _), .. } if own(prog.fns[*id].module) => Some(prog.fns[*id].name.clone()),
        TK::Try { call, .. } => called_name(prog, call, own),
        TK::Block(b) if b.stmts.is_empty() => b.tail.as_ref().and_then(|t| called_name(prog, t, own)),
        _ => None,
    }
}

/// What an expression (a handler) reads, answers and queries, following
/// the program's own functions it calls.
fn describe(prog: &Program, e: &TExpr, own: &dyn Fn(usize) -> bool, seen: &mut HashSet<usize>, r: &mut Route, depth: usize) {
    let mut calls: Vec<usize> = vec![];
    walk_expr(e, &mut |x| {
        if let Some(sql) = sql_text(prog, x) {
            if !r.sql.contains(&sql) {
                r.sql.push(sql);
            }
        }
        let fid = match &x.kind {
            TK::Call { callee: Callee::Fn(id, targs), args, .. } => {
                let f = &prog.fns[*id];
                let module = prog.module_names.get(f.module).map(String::as_str).unwrap_or("");
                let owner = f.owner.map(|o| prog.defs[o].name.as_str());
                match (module, owner, f.name.as_str()) {
                    ("json", None, "decode") if args.first().is_some_and(|a| reads_request(prog, a)) => r.requests.push(("JSON body".into(), targs.first().cloned().unwrap_or(Ty::Err))),
                    ("http", Some("Request"), "decode_form") => r.requests.push(("form".into(), targs.first().cloned().unwrap_or(Ty::Err))),
                    ("http", Some("Request"), "decode_query") => r.requests.push(("query string".into(), targs.first().cloned().unwrap_or(Ty::Err))),
                    ("http", None, "json") => r.responses.push((status(args.first()), Response::Json(targs.first().cloned().unwrap_or(Ty::Err)))),
                    ("http", None, kind @ ("text" | "html" | "redirect" | "file" | "stream" | "bytes")) => r.responses.push((status(args.first()), Response::Other(kind.to_string()))),
                    _ => {}
                }
                Some(*id)
            }
            TK::FnRef(id, _) => Some(*id),
            TK::MutCall { fn_id, .. } => Some(*fn_id),
            _ => None,
        };
        if let Some(id) = fid {
            if own(prog.fns[id].module) && prog.fns[id].body.is_some() {
                calls.push(id);
            }
        }
    });
    if depth > 12 {
        return;
    }
    for id in calls {
        if seen.insert(id) {
            if let Some(b) = &prog.fns[id].body {
                let block = TExpr { kind: TK::Block(b.block.clone()), ty: Ty::Unit, span: prog.fns[id].span };
                describe(prog, &block, own, seen, r, depth + 1);
            }
        }
    }
}

/// Whether an expression reads the request (`req.text()`, `req.body`).
fn reads_request(prog: &Program, e: &TExpr) -> bool {
    let mut found = false;
    walk_expr(e, &mut |x| {
        if let Ty::Adt(d, _) = &x.ty {
            let def = &prog.defs[*d];
            if def.name == "Request" && prog.module_names.get(def.module).map(String::as_str) == Some("http") {
                found = true;
            }
        }
    });
    found
}

fn status(e: Option<&TExpr>) -> Option<i64> {
    match e.map(|e| &e.kind) {
        Some(TK::Int(n)) => Some(*n),
        _ => None,
    }
}

fn walk_block(b: &TBlock, f: &mut dyn FnMut(&TExpr)) {
    for s in &b.stmts {
        walk_stmt(s, f);
    }
    if let Some(t) = &b.tail {
        walk_expr(t, f);
    }
}

fn walk_stmt(s: &TStmt, f: &mut dyn FnMut(&TExpr)) {
    match s {
        TStmt::Let(_, e) | TStmt::Discard(e) | TStmt::Expr(e) | TStmt::Throw(e) => walk_expr(e, f),
        TStmt::Assign(_, e) => walk_expr(e, f),
        TStmt::Return(e) => {
            if let Some(e) = e {
                walk_expr(e, f)
            }
        }
        TStmt::While(c, b) => {
            walk_expr(c, f);
            walk_block(b, f);
        }
        TStmt::ForList { list: x, body, .. } | TStmt::ForMap { map: x, body, .. } | TStmt::ForSet { set: x, body, .. } | TStmt::ForChannel { chan: x, body, .. } => {
            walk_expr(x, f);
            walk_block(body, f);
        }
        TStmt::ForRange { lo, hi, body, .. } => {
            walk_expr(lo, f);
            walk_expr(hi, f);
            walk_block(body, f);
        }
        TStmt::WithLock { shared, body, .. } => {
            walk_expr(shared, f);
            walk_block(body, f);
        }
        TStmt::With { value, body, .. } => {
            walk_expr(value, f);
            walk_block(body, f);
        }
        TStmt::Expect { cond, .. } => walk_expr(cond, f),
        TStmt::Break | TStmt::Continue => {}
    }
}

/// Calls `f` on `e` and every expression inside it.
fn walk_expr(e: &TExpr, f: &mut dyn FnMut(&TExpr)) {
    f(e);
    match &e.kind {
        TK::Call { callee, args, .. } => {
            if let Callee::Value(v) = callee {
                walk_expr(v, f);
            }
            args.iter().for_each(|a| walk_expr(a, f));
        }
        TK::MutCall { args, .. } | TK::IfaceMutCall { args, .. } => args.iter().for_each(|a| walk_expr(a, f)),
        TK::Record { fields, .. } | TK::Variant { fields, .. } | TK::List(fields) | TK::Interp(fields) => fields.iter().for_each(|a| walk_expr(a, f)),
        TK::Map(pairs) => pairs.iter().for_each(|(k, v)| {
            walk_expr(k, f);
            walk_expr(v, f);
        }),
        TK::Wrap(x) | TK::Unwrap(x) | TK::Field(x, _) | TK::Prop(x, _) | TK::Unary(_, x) | TK::Some(x) | TK::ToIface(x) | TK::ExpectThrows(x) | TK::Spawn(x) | TK::ToText(x) | TK::Is(x, _) => walk_expr(x, f),
        TK::Index(a, b) | TK::MapGet(a, b) | TK::Binary(_, a, b) | TK::Coalesce(a, b) => {
            walk_expr(a, f);
            walk_expr(b, f);
        }
        TK::Lambda { body, .. } => walk_expr(body, f),
        TK::Block(b) => walk_block(b, f),
        TK::If { cond, then, els } => {
            walk_expr(cond, f);
            walk_block(then, f);
            if let Some(e) = els {
                walk_expr(e, f);
            }
        }
        TK::Match { scrut, arms } => {
            walk_expr(scrut, f);
            for a in arms {
                if let Some(g) = &a.guard {
                    walk_expr(g, f);
                }
                walk_expr(&a.body, f);
            }
        }
        TK::Try { call, catch } => {
            walk_expr(call, f);
            if let Some((_, b)) = catch {
                walk_block(b, f);
            }
        }
        _ => {}
    }
}

// ---- writing it out

/// The records and enums a set of types mentions, by name.
fn collect_types(prog: &Program, t: &Ty, out: &mut BTreeMap<String, Ty>) {
    match t {
        Ty::Opt(x) => collect_types(prog, x, out),
        Ty::Adt(d, args) => {
            args.iter().for_each(|a| collect_types(prog, a, out));
            let def = &prog.defs[*d];
            if matches!(def.kind, TypeKind::Record { .. } | TypeKind::Enum { .. } | TypeKind::Newtype(_)) && !def.is_prelude {
                let name = prog.show(t);
                if out.insert(name, t.clone()).is_none() {
                    let fields: Vec<Ty> = match &def.kind {
                        TypeKind::Record { fields } => fields.iter().map(|f| f.ty.subst(args)).collect(),
                        TypeKind::Enum { variants } => variants.iter().flat_map(|v| v.fields.iter().map(|f| f.ty.subst(args))).collect(),
                        TypeKind::Newtype(inner) => vec![inner.subst(args)],
                        _ => vec![],
                    };
                    for f in fields {
                        collect_types(prog, &f, out);
                    }
                }
            }
        }
        _ => {}
    }
}

/// `API.md`: the routes, then the types they use.
pub fn markdown(prog: &Program, apps: &[(String, String, Vec<Route>)]) -> String {
    let mut s = String::from("# API\n\nEvery route of the project's apps, read from the code by `plumb export`: what each takes, what it answers, and the SQL it runs (its own and that of the functions it calls). Request and response bodies are JSON unless said otherwise; field names are as written (snake_case), and a field marked optional may be missing or null.\n");
    let mut types = BTreeMap::new();
    for (app, prefix, routes) in apps {
        let _ = std::fmt::Write::write_fmt(&mut s, format_args!("\n## App `{}`{}\n", app, if prefix.is_empty() || prefix == "/" { String::new() } else { format!(" (under `{}`)", prefix) }));
        if routes.is_empty() {
            s.push_str("\nNo routes found in its code (static files, or routes made from computed paths).\n");
        }
        for r in routes {
            s += &format!("\n### {} `{}`\n\nHandler: `{}` ({})\n", r.method, r.path, r.handler, r.at);
            for (how, t) in &r.requests {
                s += &format!("- takes: {} `{}`\n", how, prog.show(t));
                collect_types(prog, t, &mut types);
            }
            for (status, body) in &r.responses {
                let st = status.map(|n| n.to_string()).unwrap_or_else(|| "?".into());
                match body {
                    Response::Json(t) => {
                        s += &format!("- answers {}: JSON `{}`\n", st, prog.show(t));
                        collect_types(prog, t, &mut types);
                    }
                    Response::Other(k) => s += &format!("- answers {}: {}\n", st, k),
                }
            }
            if !r.sql.is_empty() {
                s += "- SQL:\n";
                for q in &r.sql {
                    s += &format!("  ```sql\n  {}\n  ```\n", q.trim().replace('\n', "\n  "));
                }
            }
        }
    }
    if !types.is_empty() {
        s += "\n## Types\n";
        for (name, t) in &types {
            s += &format!("\n### `{}`\n\n```\n{}```\n", name, type_body(prog, t));
        }
    }
    s
}

fn type_body(prog: &Program, t: &Ty) -> String {
    let Ty::Adt(d, args) = t else { return String::new() };
    let def = &prog.defs[*d];
    let mut s = String::new();
    match &def.kind {
        TypeKind::Record { fields } => {
            for f in fields {
                let ty = f.ty.subst(args);
                let note = match (&ty, &f.default_text) {
                    (Ty::Opt(_), _) => " (optional)".to_string(),
                    (_, Some(d)) => format!(" (optional, default {})", d),
                    _ => String::new(),
                };
                s += &format!("{}: {}{}\n", f.name, prog.show(&ty), note);
            }
        }
        TypeKind::Enum { variants } => {
            for v in variants {
                let fields: Vec<String> = v.fields.iter().map(|f| format!("{}: {}", f.name, prog.show(&f.ty.subst(args)))).collect();
                s += &format!("{}{}\n", v.name, if fields.is_empty() { String::new() } else { format!("({})", fields.join(", ")) });
            }
        }
        TypeKind::Newtype(inner) => s += &format!("a {} of its own\n", prog.show(&inner.subst(args))),
        _ => {}
    }
    s
}

fn schema(prog: &Program, t: &Ty) -> String {
    match t {
        Ty::Int => r#"{"type":"integer"}"#.into(),
        Ty::Float => r#"{"type":"number"}"#.into(),
        Ty::Bool => r#"{"type":"boolean"}"#.into(),
        Ty::Text => r#"{"type":"string"}"#.into(),
        Ty::Opt(x) => format!(r#"{{"allOf":[{}],"nullable":true}}"#, schema(prog, x)),
        Ty::Adt(d, args) => {
            let def = &prog.defs[*d];
            match (def.name.as_str(), args.as_slice()) {
                ("List" | "Set", [x]) if def.is_prelude || matches!(def.kind, TypeKind::Builtin) => format!(r#"{{"type":"array","items":{}}}"#, schema(prog, x)),
                ("Map", [_, v]) if matches!(def.kind, TypeKind::Builtin) => format!(r#"{{"type":"object","additionalProperties":{}}}"#, schema(prog, v)),
                ("Bytes", _) if matches!(def.kind, TypeKind::Builtin) => r#"{"type":"string","format":"byte"}"#.into(),
                ("Decimal", _) if matches!(def.kind, TypeKind::Builtin) => r#"{"type":"number"}"#.into(),
                _ if matches!(def.kind, TypeKind::Record { .. } | TypeKind::Enum { .. } | TypeKind::Newtype(_)) => format!(r##"{{"$ref":"#/components/schemas/{}"}}"##, ref_name(&prog.show(t))),
                _ => r#"{}"#.into(),
            }
        }
        _ => r#"{}"#.into(),
    }
}

fn ref_name(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '.' { c } else { '_' }).collect()
}

fn component(prog: &Program, t: &Ty) -> String {
    let Ty::Adt(d, args) = t else { return "{}".into() };
    match &prog.defs[*d].kind {
        TypeKind::Record { fields } => {
            let props: Vec<String> = fields.iter().map(|f| format!("{}:{}", json_str(&f.name), schema(prog, &f.ty.subst(args)))).collect();
            let required: Vec<String> = fields.iter().filter(|f| f.default.is_none() && !matches!(f.ty, Ty::Opt(_))).map(|f| json_str(&f.name)).collect();
            format!(r#"{{"type":"object","properties":{{{}}},"required":[{}]}}"#, props.join(","), required.join(","))
        }
        TypeKind::Newtype(inner) => schema(prog, &inner.subst(args)),
        TypeKind::Enum { variants } => {
            let names: Vec<String> = variants.iter().map(|v| json_str(&v.name)).collect();
            format!(r#"{{"description":"one of: {}"}}"#, names.join(", ").replace('"', ""))
        }
        _ => "{}".into(),
    }
}

fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// `openapi.json` (OpenAPI 3.0): paths with their request and response
/// bodies, and the types as schemas.
pub fn openapi(prog: &Program, title: &str, apps: &[(String, String, Vec<Route>)]) -> String {
    let mut paths: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut types = BTreeMap::new();
    for (_, _, routes) in apps {
        for r in routes.iter().filter(|r| r.method != "WEBSOCKET") {
            // `:id` → `{id}`, `*rest` → `{rest}`
            let path: String = r.path.split('/').map(|p| if let Some(n) = p.strip_prefix(':').or_else(|| p.strip_prefix('*')) { format!("{{{}}}", n) } else { p.to_string() }).collect::<Vec<_>>().join("/");
            let params: Vec<String> = r.path.split('/').filter_map(|p| p.strip_prefix(':').or_else(|| p.strip_prefix('*'))).map(|n| format!(r#"{{"name":{},"in":"path","required":true,"schema":{{"type":"string"}}}}"#, json_str(n))).collect();
            let mut op = format!(r#""summary":{},"x-handler":{}"#, json_str(&format!("{} ({})", r.handler, r.at)), json_str(&r.handler));
            if !params.is_empty() {
                op += &format!(r#","parameters":[{}]"#, params.join(","));
            }
            if let Some((how, t)) = r.requests.first() {
                collect_types(prog, t, &mut types);
                let media = match how.as_str() {
                    "form" => "application/x-www-form-urlencoded",
                    _ => "application/json",
                };
                if how != "query string" {
                    op += &format!(r#","requestBody":{{"required":true,"content":{{"{}":{{"schema":{}}}}}}}"#, media, schema(prog, t));
                }
            }
            let mut resps: BTreeMap<String, String> = BTreeMap::new();
            for (status, body) in &r.responses {
                let code = status.map(|n| n.to_string()).unwrap_or_else(|| "default".into());
                let content = match body {
                    Response::Json(t) => {
                        collect_types(prog, t, &mut types);
                        format!(r#","content":{{"application/json":{{"schema":{}}}}}"#, schema(prog, t))
                    }
                    Response::Other(k) => format!(r#","content":{{"text/plain":{{"schema":{{"type":"string","description":{}}}}}}}"#, json_str(k)),
                };
                resps.entry(code).or_insert_with(|| format!(r#"{{"description":"",{}}}"#, &content[1..]));
            }
            if resps.is_empty() {
                resps.insert("default".into(), r#"{"description":""}"#.into());
            }
            let resps: Vec<String> = resps.iter().map(|(k, v)| format!("{}:{}", json_str(k), v)).collect();
            op += &format!(r#","responses":{{{}}}"#, resps.join(","));
            let methods: Vec<&str> = if r.method == "ANY" { vec!["get", "post", "put", "patch", "delete"] } else { vec![] };
            if methods.is_empty() {
                paths.entry(path.clone()).or_default().push(format!("{}:{{{}}}", json_str(&r.method.to_lowercase()), op));
            } else {
                for m in methods {
                    paths.entry(path.clone()).or_default().push(format!("{}:{{{}}}", json_str(m), op));
                }
            }
        }
    }
    let paths: Vec<String> = paths.iter().map(|(p, ops)| format!("{}:{{{}}}", json_str(p), ops.join(","))).collect();
    let comps: Vec<String> = types.iter().map(|(n, t)| format!("{}:{}", json_str(&ref_name(n)), component(prog, t))).collect();
    format!(
        r#"{{"openapi":"3.0.3","info":{{"title":{},"version":"1"}},"paths":{{{}}},"components":{{"schemas":{{{}}}}}}}"#,
        json_str(title),
        paths.join(","),
        comps.join(",")
    ) + "\n"
}

/// SQLite-only parts of an SQL statement, each with what to use for
/// Postgres (and most other databases).
pub fn portability(sql: &str) -> Vec<&'static str> {
    let l = sql.to_lowercase();
    let mut out = vec![];
    let has = |w: &str| l.contains(w);
    if has("autoincrement") {
        out.push("AUTOINCREMENT: Postgres has `generated always as identity` (or serial)");
    }
    if has("insert or replace") || has("replace into") {
        out.push("INSERT OR REPLACE: write `insert ... on conflict (...) do update set ...`");
    }
    if has("insert or ignore") {
        out.push("INSERT OR IGNORE: write `insert ... on conflict do nothing`");
    }
    for f in ["datetime(", "strftime(", "julianday(", "unixepoch("] {
        if has(f) {
            out.push("SQLite's date functions: Postgres has now(), to_char(), extract()");
            break;
        }
    }
    if has("ifnull(") {
        out.push("ifnull(): coalesce() works everywhere");
    }
    if has("group_concat(") {
        out.push("group_concat(): Postgres has string_agg()");
    }
    if has("last_insert_rowid") {
        out.push("last_insert_rowid(): `insert ... returning id` works in both");
    }
    if has("pragma ") {
        out.push("PRAGMA: SQLite only");
    }
    if has("without rowid") {
        out.push("WITHOUT ROWID: SQLite only (drop it)");
    }
    if has(" glob ") {
        out.push("GLOB: use LIKE (or ~ in Postgres)");
    }
    if l.contains("integer primary key") && !has("identity") {
        out.push("`integer primary key` numbers rows by itself only in SQLite: Postgres needs `generated always as identity`");
    }
    out
}

/// The modules that are the program's own (not the standard library or the
/// prelude), by index.
pub fn own_modules(prog: &Program, std_names: &[&str]) -> BTreeSet<usize> {
    prog.module_names.iter().enumerate().filter(|(_, n)| !n.starts_with('<') && !std_names.contains(&n.as_str())).map(|(i, _)| i).collect()
}
