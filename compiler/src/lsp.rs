// `plumb lsp`: a language server (LSP, JSON-RPC on standard input and
// output) for editors: errors as you type, completion after `.` (a value's
// fields and methods, a module's functions and types), hover (types and
// doc comments) and go to definition. Every request checks the program
// anew with the editor's unsaved text (a check takes tens of milliseconds);
// the checker records each expression's type for it (`Checker::record`).

use crate::diag::{Sources, Span};
use crate::types::{FnDef, Program, SelfMode, Ty, TypeKind};
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------- JSON

#[derive(Clone, Debug)]
enum J {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<J>),
    Obj(Vec<(String, J)>),
}

impl J {
    fn get(&self, key: &str) -> &J {
        static NULL: J = J::Null;
        match self {
            J::Obj(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v).unwrap_or(&NULL),
            _ => &NULL,
        }
    }
    fn str(&self) -> &str {
        match self {
            J::Str(s) => s,
            _ => "",
        }
    }
    fn num(&self) -> i64 {
        match self {
            J::Num(n) => *n as i64,
            _ => 0,
        }
    }
    fn write(&self, out: &mut String) {
        match self {
            J::Null => out.push_str("null"),
            J::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            J::Num(n) => {
                if n.fract() == 0.0 && n.abs() < 1e15 {
                    out.push_str(&format!("{}", *n as i64));
                } else {
                    out.push_str(&format!("{}", n));
                }
            }
            J::Str(s) => {
                out.push('"');
                for c in s.chars() {
                    match c {
                        '"' => out.push_str("\\\""),
                        '\\' => out.push_str("\\\\"),
                        '\n' => out.push_str("\\n"),
                        '\r' => out.push_str("\\r"),
                        '\t' => out.push_str("\\t"),
                        c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
                        c => out.push(c),
                    }
                }
                out.push('"');
            }
            J::Arr(a) => {
                out.push('[');
                for (i, v) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    v.write(out);
                }
                out.push(']');
            }
            J::Obj(m) => {
                out.push('{');
                for (i, (k, v)) in m.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    J::Str(k.clone()).write(out);
                    out.push(':');
                    v.write(out);
                }
                out.push('}');
            }
        }
    }
}

fn obj(fields: Vec<(&str, J)>) -> J {
    J::Obj(fields.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

fn s(v: impl Into<String>) -> J {
    J::Str(v.into())
}

fn n(v: i64) -> J {
    J::Num(v as f64)
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.i < self.b.len() && (self.b[self.i] as char).is_ascii_whitespace() {
            self.i += 1;
        }
    }
    fn value(&mut self) -> Option<J> {
        self.ws();
        let c = *self.b.get(self.i)?;
        match c {
            b'{' => {
                self.i += 1;
                let mut m = vec![];
                loop {
                    self.ws();
                    if self.b.get(self.i) == Some(&b'}') {
                        self.i += 1;
                        return Some(J::Obj(m));
                    }
                    let k = match self.value()? {
                        J::Str(k) => k,
                        _ => return None,
                    };
                    self.ws();
                    if self.b.get(self.i) != Some(&b':') {
                        return None;
                    }
                    self.i += 1;
                    let v = self.value()?;
                    m.push((k, v));
                    self.ws();
                    match self.b.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b'}') => {}
                        _ => return None,
                    }
                }
            }
            b'[' => {
                self.i += 1;
                let mut a = vec![];
                loop {
                    self.ws();
                    if self.b.get(self.i) == Some(&b']') {
                        self.i += 1;
                        return Some(J::Arr(a));
                    }
                    a.push(self.value()?);
                    self.ws();
                    match self.b.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b']') => {}
                        _ => return None,
                    }
                }
            }
            b'"' => {
                self.i += 1;
                let mut out = String::new();
                loop {
                    let c = *self.b.get(self.i)?;
                    self.i += 1;
                    match c {
                        b'"' => return Some(J::Str(out)),
                        b'\\' => {
                            let e = *self.b.get(self.i)?;
                            self.i += 1;
                            match e {
                                b'n' => out.push('\n'),
                                b't' => out.push('\t'),
                                b'r' => out.push('\r'),
                                b'b' => out.push('\u{8}'),
                                b'f' => out.push('\u{c}'),
                                b'u' => {
                                    let hex = std::str::from_utf8(self.b.get(self.i..self.i + 4)?).ok()?;
                                    let mut cp = u32::from_str_radix(hex, 16).ok()?;
                                    self.i += 4;
                                    // a surrogate pair
                                    if (0xd800..0xdc00).contains(&cp) && self.b.get(self.i) == Some(&b'\\') && self.b.get(self.i + 1) == Some(&b'u') {
                                        let lo = u32::from_str_radix(std::str::from_utf8(self.b.get(self.i + 2..self.i + 6)?).ok()?, 16).ok()?;
                                        self.i += 6;
                                        cp = 0x10000 + ((cp - 0xd800) << 10) + (lo - 0xdc00);
                                    }
                                    out.push(char::from_u32(cp).unwrap_or('\u{fffd}'));
                                }
                                other => out.push(other as char),
                            }
                        }
                        _ => {
                            // the rest of a UTF-8 sequence comes as it is
                            let start = self.i - 1;
                            let mut end = self.i;
                            while end < self.b.len() && (self.b[end] & 0xc0) == 0x80 {
                                end += 1;
                            }
                            out.push_str(std::str::from_utf8(&self.b[start..end]).unwrap_or("\u{fffd}"));
                            self.i = end;
                        }
                    }
                }
            }
            b't' if self.b[self.i..].starts_with(b"true") => {
                self.i += 4;
                Some(J::Bool(true))
            }
            b'f' if self.b[self.i..].starts_with(b"false") => {
                self.i += 5;
                Some(J::Bool(false))
            }
            b'n' if self.b[self.i..].starts_with(b"null") => {
                self.i += 4;
                Some(J::Null)
            }
            _ => {
                let start = self.i;
                while self.i < self.b.len() && matches!(self.b[self.i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') {
                    self.i += 1;
                }
                std::str::from_utf8(&self.b[start..self.i]).ok()?.parse().ok().map(J::Num)
            }
        }
    }
}

fn parse_json(text: &str) -> Option<J> {
    Parser { b: text.as_bytes(), i: 0 }.value()
}

// ---------------------------------------------------------------- transport

fn read_message(input: &mut impl BufRead) -> Option<J> {
    let mut len = None;
    loop {
        let mut line = String::new();
        if input.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(v) = line.strip_prefix("Content-Length:") {
            len = v.trim().parse::<usize>().ok();
        }
    }
    let mut buf = vec![0u8; len?];
    input.read_exact(&mut buf).ok()?;
    parse_json(&String::from_utf8_lossy(&buf))
}

fn send(msg: J) {
    let mut body = String::new();
    msg.write(&mut body);
    let mut out = std::io::stdout().lock();
    let _ = write!(out, "Content-Length: {}\r\n\r\n{}", body.len(), body);
    let _ = out.flush();
}

fn reply(id: &J, result: J) {
    send(obj(vec![("jsonrpc", s("2.0")), ("id", id.clone()), ("result", result)]));
}

fn notify(method: &str, params: J) {
    send(obj(vec![("jsonrpc", s("2.0")), ("method", s(method)), ("params", params)]));
}

// ---------------------------------------------------------------- positions

fn uri_to_path(uri: &str) -> PathBuf {
    let p = uri.strip_prefix("file://").unwrap_or(uri);
    // percent-decoding (spaces, non-ASCII names)
    let b = p.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&p[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    PathBuf::from(String::from_utf8_lossy(&out).into_owned())
}

fn path_to_uri(p: &Path) -> String {
    let mut out = String::from("file://");
    for c in p.to_string_lossy().chars() {
        match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '/' | '-' | '_' | '.' | '~' => out.push(c),
            c => {
                let mut buf = [0u8; 4];
                for b in c.encode_utf8(&mut buf).bytes() {
                    out.push_str(&format!("%{:02X}", b));
                }
            }
        }
    }
    out
}

// LSP positions count UTF-16 units within a line
fn offset_to_pos(text: &str, off: usize) -> (i64, i64) {
    let off = off.min(text.len());
    let before = &text[..off];
    let line = before.matches('\n').count();
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let col: usize = text[line_start..off].chars().map(|c| c.len_utf16()).sum();
    (line as i64, col as i64)
}

fn pos_to_offset(text: &str, line: i64, col: i64) -> usize {
    let mut off = 0;
    for _ in 0..line {
        match text[off..].find('\n') {
            Some(i) => off += i + 1,
            None => return text.len(),
        }
    }
    let mut units = 0;
    for (i, c) in text[off..].char_indices() {
        if units >= col as usize || c == '\n' {
            return off + i;
        }
        units += c.len_utf16();
    }
    text.len()
}

fn range(text: &str, lo: usize, hi: usize) -> J {
    let (l1, c1) = offset_to_pos(text, lo);
    let (l2, c2) = offset_to_pos(text, hi);
    obj(vec![("start", obj(vec![("line", n(l1)), ("character", n(c1))])), ("end", obj(vec![("line", n(l2)), ("character", n(c2))]))])
}

// ---------------------------------------------------------------- analysis

struct Analysis {
    sources: Sources,
    checker: Option<crate::check::Checker>,
    // the analyzed file's index in `sources`
    file: u32,
    // errors that stopped it before checking
    load_errors: Vec<crate::diag::Diag>,
}

fn analyze(path: &Path, text: &str, docs: &HashMap<PathBuf, String>) -> Analysis {
    let mut overrides = docs.clone();
    overrides.insert(path.to_path_buf(), text.to_string());
    let mut sources = Sources::default();
    // a checker that panics on half-written code mustn't take the server down
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| crate::analyze(&path.to_string_lossy(), &mut sources, &overrides, true)));
    match result {
        Ok(Ok((c, uf))) => Analysis { sources, checker: Some(c), file: uf, load_errors: vec![] },
        // the user's file comes right after the prelude
        Ok(Err(errs)) => Analysis { sources, checker: None, file: 1, load_errors: errs.unwrap_or_default() },
        Err(_) => Analysis { sources: Sources::default(), checker: None, file: 1, load_errors: vec![] },
    }
}

// For completion and hover while typing: a line that doesn't parse
// (half-written elsewhere in the file) is blanked out, offsets kept, and the
// check tried again, up to three lines.
fn analyze_tolerant(path: &Path, text: &str, docs: &HashMap<PathBuf, String>) -> Analysis {
    let mut text = text.to_string();
    let mut a = analyze(path, &text, docs);
    for _ in 0..3 {
        if a.checker.is_some() {
            break;
        }
        let Some(d) = a.load_errors.iter().find(|d| d.span.file == a.file) else { break };
        let at = (d.span.lo as usize).min(text.len());
        let lo = text[..at].rfind('\n').map_or(0, |i| i + 1);
        let hi = text[at..].find('\n').map_or(text.len(), |i| at + i);
        if text[lo..hi].trim().is_empty() {
            break;
        }
        let blank: String = text[lo..hi].chars().map(|c| if c == '\t' { '\t' } else { ' ' }).collect();
        // multi-byte characters become one space each: keep byte offsets
        let blank: String = blank.chars().zip(text[lo..hi].chars()).map(|(b, c)| b.to_string().repeat(c.len_utf8())).collect();
        text.replace_range(lo..hi, &blank);
        a = analyze(path, &text, docs);
    }
    a
}

fn diagnostics(path: &Path, text: &str, docs: &HashMap<PathBuf, String>) -> J {
    let a = analyze(path, text, docs);
    let ds = match &a.checker {
        Some(c) => crate::checked_diags(c),
        None => a.load_errors.clone(),
    };
    let mut out = vec![];
    for d in ds {
        let mut msg = d.msg.clone();
        if let Some(h) = &d.help {
            msg += &format!("\n{}", h);
        }
        let r = if d.span.file == a.file {
            range(text, d.span.lo as usize, d.span.hi.max(d.span.lo + 1) as usize)
        } else {
            // in an imported file: on the first line, naming it
            let file = a.sources.files.get(d.span.file as usize).map(|f| f.path.clone()).unwrap_or_default();
            msg = format!("{}: {}", file, msg);
            range(text, 0, 0)
        };
        out.push(obj(vec![("range", r), ("severity", n(1)), ("source", s("plumb")), ("message", s(msg))]));
    }
    J::Arr(out)
}

// The comment lines right above a definition, without the `//`.
fn doc_above(text: &str, at: usize) -> String {
    let line_start = text[..at.min(text.len())].rfind('\n').map_or(0, |i| i + 1);
    let mut lines = vec![];
    let mut end = line_start;
    while end > 0 {
        let prev_start = text[..end - 1].rfind('\n').map_or(0, |i| i + 1);
        let line = text[prev_start..end - 1].trim();
        match line.strip_prefix("//") {
            Some(c) => lines.push(c.strip_prefix(' ').unwrap_or(c).to_string()),
            None => break,
        }
        end = prev_start;
    }
    lines.reverse();
    lines.join("\n")
}

// A type as written, with type parameters by name (`T`, not `T0`).
fn show(prog: &Program, t: &Ty, generics: &[String]) -> String {
    match t {
        Ty::Param(i) => generics.get(*i as usize).cloned().unwrap_or_else(|| prog.show(t)),
        Ty::Adt(d, args) if !args.is_empty() => format!("{}<{}>", prog.defs[*d].name, args.iter().map(|a| show(prog, a, generics)).collect::<Vec<_>>().join(", ")),
        Ty::Opt(inner) if !matches!(**inner, Ty::Func(_)) => format!("{}?", show(prog, inner, generics)),
        Ty::Func(f) => {
            let mut out = format!("fn({})", f.params.iter().map(|a| show(prog, a, generics)).collect::<Vec<_>>().join(", "));
            if f.throws {
                out += " throws";
            }
            if !matches!(f.ret, Ty::Unit) {
                out += &format!(" -> {}", show(prog, &f.ret, generics));
            }
            out
        }
        _ => prog.show(t),
    }
}

fn signature(prog: &Program, f: &FnDef) -> String {
    let params: Vec<String> = f.params.iter().filter(|(n, _)| n != "self").map(|(n, t)| format!("{}: {}", n, show(prog, t, &f.generics))).collect();
    let generics = {
        let own: Vec<&String> = match f.owner {
            Some(o) => f.generics.iter().skip(prog.defs[o].generics.len()).collect(),
            None => f.generics.iter().collect(),
        };
        if own.is_empty() { String::new() } else { format!("<{}>", own.iter().map(|g| g.as_str()).collect::<Vec<_>>().join(", ")) }
    };
    let mut sig = format!("fn {}{}({})", f.name, generics, params.join(", "));
    if f.throws {
        sig += " throws";
    }
    if !matches!(f.ret, Ty::Unit) {
        sig += &format!(" -> {}", show(prog, &f.ret, &f.generics));
    }
    sig
}

struct Member {
    name: String,
    // LSP CompletionItemKind
    kind: i64,
    detail: String,
    doc: String,
    span: Option<Span>,
    // a method or function: completed with `()`
    call: bool,
    has_params: bool,
}

fn source_doc(a: &Analysis, span: Span) -> String {
    a.sources.files.get(span.file as usize).map(|f| doc_above(&f.text, span.lo as usize)).unwrap_or_default()
}

// The definition behind a type: List, String, Int... and records.
fn def_of(prog: &Program, t: &Ty) -> Option<usize> {
    let name = match t {
        Ty::Adt(d, _) => return Some(*d),
        Ty::Opt(inner) => return def_of(prog, inner),
        Ty::Int => "Int",
        Ty::Float => "Float",
        Ty::Bool => "Bool",
        Ty::Text => "String",
        _ => return None,
    };
    prog.defs.iter().position(|d| d.name == name && d.is_prelude)
}

fn members_of_type(a: &Analysis, t: &Ty) -> Vec<Member> {
    let prog = &a.checker.as_ref().unwrap().prog;
    let mut out = vec![];
    let Some(d) = def_of(prog, t) else { return out };
    let def = &prog.defs[d];
    if let TypeKind::Record { fields } = &def.kind {
        for f in fields {
            out.push(Member { name: f.name.clone(), kind: 5, detail: format!("{}: {}", f.name, prog.show(&f.ty)), doc: f.doc.clone().unwrap_or_default(), span: Some(f.span), call: false, has_params: false });
        }
    }
    for (name, ty) in &def.props {
        out.push(Member { name: name.clone(), kind: 10, detail: format!("{}: {}", name, prog.show(ty)), doc: String::new(), span: None, call: false, has_params: false });
    }
    let mut methods: Vec<(&String, &usize)> = def.methods.iter().collect();
    methods.sort();
    for (name, id) in methods {
        let f = &prog.fns[*id];
        // instance methods only; `__` ones are the library's own
        if f.self_mode == SelfMode::None || name.starts_with("__") {
            continue;
        }
        let has_params = f.params.iter().any(|(n, _)| n != "self");
        out.push(Member { name: name.clone(), kind: 2, detail: signature(prog, f), doc: source_doc(a, f.span), span: Some(f.span), call: true, has_params });
    }
    out
}

// A module's public functions, types and constants: `json.` → decode, encode...
fn members_of_module(a: &Analysis, module: &str) -> Vec<Member> {
    let prog = &a.checker.as_ref().unwrap().prog;
    let Some(m) = prog.module_names.iter().position(|k| k == module || k.ends_with(&format!(".{}", module))) else { return vec![] };
    let mut out = vec![];
    for f in &prog.fns {
        if f.module != m || f.owner.is_some() || !f.is_pub || f.name.starts_with("__") {
            continue;
        }
        if f.is_const {
            out.push(Member { name: f.name.clone(), kind: 21, detail: format!("let {}: {}", f.name, prog.show(&f.ret)), doc: source_doc(a, f.span), span: Some(f.span), call: false, has_params: false });
        } else {
            let has_params = !f.params.is_empty();
            out.push(Member { name: f.name.clone(), kind: 3, detail: signature(prog, f), doc: source_doc(a, f.span), span: Some(f.span), call: true, has_params });
        }
    }
    for d in &prog.defs {
        if d.module != m || !d.is_pub {
            continue;
        }
        let kind = match d.kind {
            TypeKind::Enum { .. } => 13,
            TypeKind::Interface { .. } => 8,
            _ => 22,
        };
        out.push(Member { name: d.name.clone(), kind, detail: format!("type {}", d.name), doc: source_doc(a, d.span), span: Some(d.span), call: false, has_params: false });
    }
    out.sort_by(|x, y| x.name.cmp(&y.name));
    out
}

fn ident_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

// Where the expression before `.` at `dot` starts: names, dots, `?.`, and
// balanced (...) [...] groups, as in `users[0].address?.city`.
fn receiver_start(text: &[u8], dot: usize) -> usize {
    let mut i = dot;
    loop {
        if i == 0 {
            return 0;
        }
        let c = text[i - 1];
        if ident_char(c) {
            while i > 0 && ident_char(text[i - 1]) {
                i -= 1;
            }
        } else if c == b')' || c == b']' {
            let (open, close) = if c == b')' { (b'(', b')') } else { (b'[', b']') };
            let mut depth = 0;
            while i > 0 {
                i -= 1;
                if text[i] == close {
                    depth += 1;
                } else if text[i] == open {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
            }
            continue;
        } else if c == b'"' {
            // a string literal: back to its start
            i -= 1;
            while i > 0 && !(text[i - 1] == b'"' && (i < 2 || text[i - 2] != b'\\')) {
                i -= 1;
            }
            i = i.saturating_sub(1);
            continue;
        } else {
            return i;
        }
        // more of the chain: `.name`, `?.name`
        if i > 0 && text[i - 1] == b'.' {
            i -= 1;
            if i > 0 && text[i - 1] == b'?' {
                i -= 1;
            }
            continue;
        }
        return i;
    }
}

// The members after `x.` at `dot`: the type of `x` from a check of the text
// with `.partial` blanked out (so it parses), or a module's.
fn members_at_dot(path: &Path, text: &str, dot: usize, end: usize, docs: &HashMap<PathBuf, String>) -> Option<(Analysis, Vec<Member>)> {
    let bytes = text.as_bytes();
    // `?.`: the receiver ends before the `?`
    let recv_end = if dot > 0 && bytes[dot - 1] == b'?' { dot - 1 } else { dot };
    let start = receiver_start(bytes, recv_end);
    let receiver = &text[start..recv_end];
    // the rest of a name the cursor is in the middle of goes too
    let mut end = end;
    while end < bytes.len() && ident_char(bytes[end]) {
        end += 1;
    }
    let mut blanked = text.as_bytes().to_vec();
    for b in &mut blanked[recv_end..end] {
        *b = b' ';
    }
    let blanked = String::from_utf8(blanked).ok()?;
    let a = analyze_tolerant(path, &blanked, docs);
    let c = a.checker.as_ref()?;
    // a module: `json.`
    if receiver.bytes().all(ident_char) && !receiver.is_empty() {
        let is_module = text.lines().any(|l| {
            let l = l.trim();
            l.strip_prefix("import ").is_some_and(|m| m.trim() == receiver || m.trim().ends_with(&format!(".{}", receiver)))
        });
        if is_module {
            let ms = members_of_module(&a, receiver);
            return Some((a, ms));
        }
    }
    let rec = c.record.as_ref()?;
    let ty = rec
        .iter()
        .filter(|(sp, _)| sp.file == a.file && sp.hi as usize == recv_end)
        .min_by_key(|(sp, _)| ((sp.lo as usize).abs_diff(start), u32::MAX - (sp.hi - sp.lo)))
        .map(|(_, t)| t.clone())?;
    let ms = members_of_type(&a, &ty);
    Some((a, ms))
}

const KEYWORDS: &[&str] = &[
    "fn", "let", "var", "if", "else", "for", "in", "while", "match", "return", "break", "continue", "type", "enum", "interface", "implements", "import", "pub", "try", "catch", "throw",
    "throws", "with", "spawn", "test", "expect", "true", "false", "none", "some", "and", "or", "not", "is", "self", "mutating",
];

fn completion(path: &Path, text: &str, off: usize, docs: &HashMap<PathBuf, String>) -> J {
    let bytes = text.as_bytes();
    let mut start = off;
    while start > 0 && ident_char(bytes[start - 1]) {
        start -= 1;
    }
    let mut items = vec![];
    let item = |m: &Member| {
        let mut fields = vec![("label", s(m.name.clone())), ("kind", n(m.kind)), ("detail", s(m.detail.clone()))];
        if !m.doc.is_empty() {
            fields.push(("documentation", obj(vec![("kind", s("markdown")), ("value", s(m.doc.clone()))])));
        }
        if m.call {
            fields.push(("insertText", s(if m.has_params { format!("{}($0)", m.name) } else { format!("{}()", m.name) })));
            fields.push(("insertTextFormat", n(2)));
        }
        obj(fields)
    };
    if start > 0 && bytes[start - 1] == b'.' {
        if let Some((_, members)) = members_at_dot(path, text, start - 1, off, docs) {
            items = members.iter().map(item).collect();
        }
        return obj(vec![("isIncomplete", J::Bool(false)), ("items", J::Arr(items))]);
    }
    // `import ...`: the standard modules
    let line_start = text[..off].rfind('\n').map_or(0, |i| i + 1);
    if text[line_start..off].trim_start().starts_with("import") {
        for m in crate::STD_NAMES {
            items.push(obj(vec![("label", s(*m)), ("kind", n(9)), ("detail", s("standard library"))]));
        }
        return obj(vec![("isIncomplete", J::Bool(false)), ("items", J::Arr(items))]);
    }
    for k in KEYWORDS {
        items.push(obj(vec![("label", s(*k)), ("kind", n(14))]));
    }
    // the imported modules, and this file's and the prelude's functions and types
    for l in text.lines() {
        if let Some(m) = l.trim().strip_prefix("import ") {
            let name = m.trim().rsplit('.').next().unwrap_or("").to_string();
            items.push(obj(vec![("label", s(name)), ("kind", n(9)), ("detail", s(format!("import {}", m.trim())))]));
        }
    }
    let a = analyze_tolerant(path, text, docs);
    if let Some(c) = &a.checker {
        let prog = &c.prog;
        let here = prog.module_names.len().saturating_sub(1);
        for f in &prog.fns {
            if f.owner.is_none() && !f.name.starts_with("__") && (f.module == here || (f.is_prelude && f.is_pub)) && f.name != "main" {
                let m = Member { name: f.name.clone(), kind: if f.is_const { 21 } else { 3 }, detail: if f.is_const { format!("let {}", f.name) } else { signature(prog, f) }, doc: source_doc(&a, f.span), span: None, call: !f.is_const, has_params: !f.params.is_empty() };
                items.push(item(&m));
            }
        }
        for d in &prog.defs {
            if d.module == here || (d.is_prelude && d.is_pub) {
                items.push(obj(vec![("label", s(d.name.clone())), ("kind", n(22)), ("detail", s(format!("type {}", d.name)))]));
            }
        }
    }
    obj(vec![("isIncomplete", J::Bool(false)), ("items", J::Arr(items))])
}

// The name under the cursor and where it is.
fn word_at(text: &str, off: usize) -> Option<(usize, usize)> {
    let b = text.as_bytes();
    let mut lo = off.min(b.len());
    let mut hi = lo;
    while lo > 0 && ident_char(b[lo - 1]) {
        lo -= 1;
    }
    while hi < b.len() && ident_char(b[hi]) {
        hi += 1;
    }
    (hi > lo).then_some((lo, hi))
}

// What the name at `off` is: a member after `.`, or a function or type of
// this file or the prelude.
fn lookup(path: &Path, text: &str, off: usize, docs: &HashMap<PathBuf, String>) -> Option<(Analysis, Option<Member>, usize, usize)> {
    let (lo, hi) = word_at(text, off)?;
    let word = &text[lo..hi];
    let b = text.as_bytes();
    if lo > 0 && b[lo - 1] == b'.' {
        let (a, members) = members_at_dot(path, text, lo - 1, hi, docs)?;
        let m = members.into_iter().find(|m| m.name == word);
        return Some((a, m, lo, hi));
    }
    let a = analyze_tolerant(path, text, docs);
    let found = a.checker.as_ref().and_then(|c| {
        let prog = &c.prog;
        let here = prog.module_names.len().saturating_sub(1);
        if let Some(f) = prog.fns.iter().find(|f| f.name == word && f.owner.is_none() && (f.module == here || f.is_prelude)) {
            return Some(Member { name: f.name.clone(), kind: 3, detail: if f.is_const { format!("let {}: {}", f.name, prog.show(&f.ret)) } else { signature(prog, f) }, doc: source_doc(&a, f.span), span: Some(f.span), call: true, has_params: false });
        }
        prog.defs.iter().find(|d| d.name == word && (d.module == here || d.is_prelude)).map(|d| Member { name: d.name.clone(), kind: 22, detail: format!("type {}", d.name), doc: source_doc(&a, d.span), span: Some(d.span), call: false, has_params: false })
    });
    Some((a, found, lo, hi))
}

fn hover(path: &Path, text: &str, off: usize, docs: &HashMap<PathBuf, String>) -> J {
    let Some((a, member, lo, hi)) = lookup(path, text, off, docs) else { return J::Null };
    let mut md = String::new();
    if let Some(m) = member {
        md = format!("```plumb\n{}\n```", m.detail);
        if !m.doc.is_empty() {
            md += &format!("\n\n{}", m.doc);
        }
    } else if let Some(c) = &a.checker {
        // the innermost expression here: a local, a literal, a call
        if let Some((_, t)) = c.record.as_ref().and_then(|r| r.iter().filter(|(sp, _)| sp.file == a.file && sp.lo as usize <= lo && hi <= sp.hi as usize).min_by_key(|(sp, _)| sp.hi - sp.lo)) {
            if !matches!(t, Ty::Err) {
                md = format!("```plumb\n{}: {}\n```", &text[lo..hi], c.prog.show(t));
            }
        }
    }
    if md.is_empty() {
        return J::Null;
    }
    obj(vec![("contents", obj(vec![("kind", s("markdown")), ("value", s(md))])), ("range", range(text, lo, hi))])
}

fn definition(path: &Path, text: &str, off: usize, docs: &HashMap<PathBuf, String>) -> J {
    let Some((a, Some(m), _, _)) = lookup(path, text, off, docs) else { return J::Null };
    let Some(sp) = m.span else { return J::Null };
    let Some(file) = a.sources.files.get(sp.file as usize) else { return J::Null };
    // the standard library and the prelude live inside the compiler
    if file.path.starts_with('<') {
        return J::Null;
    }
    let target = if sp.file == a.file { path.to_path_buf() } else { std::fs::canonicalize(&file.path).unwrap_or_else(|_| PathBuf::from(&file.path)) };
    obj(vec![("uri", s(path_to_uri(&target))), ("range", range(&file.text, sp.lo as usize, sp.hi as usize))])
}

// ---------------------------------------------------------------- the loop

pub fn run() -> std::process::ExitCode {
    // the reader: so edits that pile up while a check runs are checked once
    let (tx, rx) = std::sync::mpsc::channel::<J>();
    std::thread::spawn(move || {
        let mut input = std::io::BufReader::new(std::io::stdin());
        while let Some(m) = read_message(&mut input) {
            if tx.send(m).is_err() {
                break;
            }
        }
    });
    let mut docs: HashMap<PathBuf, String> = HashMap::new();
    let mut shutdown = false;
    while let Ok(first) = rx.recv() {
        let mut batch = vec![first];
        while let Ok(m) = rx.try_recv() {
            batch.push(m);
        }
        let mut changed: Vec<(String, PathBuf)> = vec![];
        for msg in batch {
            let method = msg.get("method").str().to_string();
            let id = msg.get("id").clone();
            let params = msg.get("params");
            let doc_path = || uri_to_path(params.get("textDocument").get("uri").str());
            let at = |docs: &HashMap<PathBuf, String>| {
                let p = doc_path();
                let text = docs.get(&p).cloned().or_else(|| std::fs::read_to_string(&p).ok()).unwrap_or_default();
                let pos = params.get("position");
                let off = pos_to_offset(&text, pos.get("line").num(), pos.get("character").num());
                (p, text, off)
            };
            match method.as_str() {
                "initialize" => reply(
                    &id,
                    obj(vec![
                        (
                            "capabilities",
                            obj(vec![
                                ("textDocumentSync", n(1)),
                                ("completionProvider", obj(vec![("triggerCharacters", J::Arr(vec![s(".")]))])),
                                ("hoverProvider", J::Bool(true)),
                                ("definitionProvider", J::Bool(true)),
                            ]),
                        ),
                        ("serverInfo", obj(vec![("name", s("plumb")), ("version", s(env!("CARGO_PKG_VERSION")))])),
                    ]),
                ),
                "shutdown" => {
                    shutdown = true;
                    reply(&id, J::Null);
                }
                "exit" => return if shutdown { std::process::ExitCode::SUCCESS } else { std::process::ExitCode::from(1) },
                "textDocument/didOpen" => {
                    let doc = params.get("textDocument");
                    let p = uri_to_path(doc.get("uri").str());
                    docs.insert(p.clone(), doc.get("text").str().to_string());
                    changed.push((doc.get("uri").str().to_string(), p));
                }
                "textDocument/didChange" => {
                    let p = doc_path();
                    if let J::Arr(changes) = params.get("contentChanges") {
                        if let Some(last) = changes.last() {
                            docs.insert(p.clone(), last.get("text").str().to_string());
                        }
                    }
                    changed.push((params.get("textDocument").get("uri").str().to_string(), p));
                }
                "textDocument/didSave" => changed.push((params.get("textDocument").get("uri").str().to_string(), doc_path())),
                "textDocument/didClose" => {
                    let p = doc_path();
                    docs.remove(&p);
                    notify("textDocument/publishDiagnostics", obj(vec![("uri", params.get("textDocument").get("uri").clone()), ("diagnostics", J::Arr(vec![]))]));
                }
                "textDocument/completion" => {
                    let (p, text, off) = at(&docs);
                    reply(&id, completion(&p, &text, off, &docs));
                }
                "textDocument/hover" => {
                    let (p, text, off) = at(&docs);
                    reply(&id, hover(&p, &text, off, &docs));
                }
                "textDocument/definition" => {
                    let (p, text, off) = at(&docs);
                    reply(&id, definition(&p, &text, off, &docs));
                }
                _ => {
                    // a request we don't answer: say so; notifications are ignored
                    if !matches!(id, J::Null) {
                        send(obj(vec![("jsonrpc", s("2.0")), ("id", id), ("error", obj(vec![("code", n(-32601)), ("message", s(format!("not supported: {}", method)))]))]));
                    }
                }
            }
        }
        changed.sort();
        changed.dedup();
        for (uri, p) in changed {
            if let Some(text) = docs.get(&p).cloned() {
                let ds = diagnostics(&p, &text, &docs);
                notify("textDocument/publishDiagnostics", obj(vec![("uri", s(uri)), ("diagnostics", ds)]));
            }
        }
    }
    std::process::ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_round_trip() {
        let j = parse_json(r#"{"a": [1, 2.5, "x\"yé", true, null], "b": {"c": "ü"}}"#).unwrap();
        let mut out = String::new();
        j.write(&mut out);
        assert_eq!(out, r#"{"a":[1,2.5,"x\"yé",true,null],"b":{"c":"ü"}}"#);
    }

    #[test]
    fn positions() {
        let t = "ab\nцд😀x\n";
        assert_eq!(offset_to_pos(t, 3), (1, 0));
        let x = t.find('x').unwrap();
        assert_eq!(offset_to_pos(t, x), (1, 4));
        assert_eq!(pos_to_offset(t, 1, 4), x);
    }

    #[test]
    fn receivers() {
        let t = b"let a = users[0].address?.city.";
        assert_eq!(&t[receiver_start(t, t.len() - 1)..t.len() - 1], b"users[0].address?.city");
        let t = b"f(x, conn.";
        assert_eq!(&t[receiver_start(t, t.len() - 1)..t.len() - 1], b"conn");
        let t = b"\"hi\".";
        assert_eq!(&t[receiver_start(t, t.len() - 1)..t.len() - 1], b"\"hi\"");
    }
}
