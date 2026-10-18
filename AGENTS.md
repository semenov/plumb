# Writing Plumb: a guide for agents

Everything needed to write correct code on the first try (`plumb guide`
prints this). Look up any library from the command line:

```
plumb doc                     the modules
plumb doc http                one module's API, with its comments
plumb doc http.Router         one type or function
plumb doc List.map            built-in types: List, Map, Set, String, Bytes, Int, ...
plumb doc --search gzip       find by name or description
plumb guide errors            one section of this guide
```

`plumb doc` also shows the packages of the project you're in. In the
language's repository, the same reference is in `STDLIB.md` and the
reasons behind the rules are in `DESIGN.md`.

## Files and programs

- A file is a module. A program is a file with `fn main()`. Run it: `plumb run app.plumb`. Tests: `plumb test` (the whole project).
- `import json`, `import store.users` (the file `store/users.plumb` from the
  project root). Use names with the module prefix: `json.decode<T>(text)`,
  `users.find(id)`. No `from`, no `*`, no relative paths.
- Without `pub`, a function or type is private to its file.
- A local variable can't have the name of an imported module (`let path = ...`
  with `import path` is an error).
- The top level holds `import`, `type`, `enum`, `fn`, `test`
  and **`let` for fixed values**: `let max_retries = 5`,
  `pub let default_port = 8080`, `let usage = """..."""`. The value is
  made of literals, other top-level `let`s, and lists, maps and records of
  them; no function calls. **There are no global variables**: state that
  changes is passed along, and state requests share is in the database.
- Files: `files.read` / `write` / `append` (whole files), `files.open` /
  `create` / `open_append` (streams, with `with`), `list`, `walk`, `glob`,
  `exists`, `is_dir`, `make_dir`, `copy`, `rename`, `delete`, `delete_all`,
  `temp_dir`. Paths as text: the `path` module. Failures to handle:
  `if err is files.NotFound` (also `PermissionDenied`, `IsADirectory`,
  `AlreadyExists`).

## Syntax at a glance

```
// comments with //
let x = 5                      // constant
var total = 0                  // variable
total += x
let name: String = "Ada"         // types: Int, Float, Bool, String, Bytes,
                               // List<T>, Map<K, V>, Set<T>, T?
let greeting = "Hi ${name}!"   // interpolation is ${...}; escapes: \n \t
                               // \" \\ \$ \u{1F600}
let long = """
  Text over several lines
  """                          // multi-line text: triple quotes
print(greeting)                // a line to standard output
eprint("warning: ...")         // a line to standard error
let ok = a > 0 and not done or b // and / or / not, never && || !

if x > 3 {
  ...
} else if x == 3 {
  ...
} else {
  ...
}

for item in items { ... }
if ready and user is some(u) { ... }   // `is some(...)` also after `and`
for i in 0..<10 { ... }        // 0..<n excludes n, 1..=n includes it
while running { ... }
for entry in map { print("${entry.key}: ${entry.value}") }
for item in list.indexed() { print("${item.index}: ${item.value}") }

var names: List<String> = []     // empty literals need a type: [] and {}
names.append("Ada")
var ages: Map<String, Int> = {}
ages["Ada"] = 36               // add or replace
let age = ages["Ada"] ?? 0     // reading gives Int?
ages.remove("Ada")
let counts = words.count_each()               // Map<String, Int>
let top = counts.entries().sorted_by(e => e.value).reversed().take(10)
let byname = people.sorted_by(p => [p.last, p.first])  // lists compare in turn

let size = if n > 100 { "big" } else { "small" }   // if and match give values
let label = match shape {
  Circle(radius) => "circle ${radius}"
  Rect(width, height) => "rect"
}
let kind = match word {
  "if" | "else" | "match" => "keyword"   // `|`: any of them (binds no names)
  _ => "name"
}
```

Patterns: literals, `_`, a name, `some(p)`, `none`, `Variant(p, ...)`, a
an error type (`NotFound(id)`), and `p | q`.

No semicolons. Braces always. `if` conditions have no parentheses.
Two-space indentation; `plumb fmt` lays files out the standard way.

## Types

```
type User {                    // a record: a value, copied on assignment
  id: Int
  name: String
  email: String? = none          // optional, with a default
  active: Bool = true          // default: may be left out when building
}
let u = User(id: 1, name: "Ada")   // fields are always named

enum Status {                  // variants, with or without data
  Draft
  Published(at: String)
}
let s = Status.Published(at: "today")   // a variant with its enum's name
let v = json.Value.Null                 // from another module: module.Enum.Variant
let shown = match s {                   // in a pattern the enum is known:
  Draft => "draft"                      // the bare name
  Published(at) => "since ${at}"
}

type Money {
  cents: Int
  fn to_string(self) -> String {           // its text in "${m}" (also inside
    return "$${self.cents.div(100)}"       // lists, fields and maps); without
  }                                        // it: Money(cents: 1250)
}
// an error in "${err}" is its message(); durations print as 1.5s, dates as ISO

type Mailer {                  // behavior to swap (a fake in tests): a
  send: fn(String)      // record of functions; there are no
}                              // interfaces except `Error`
let fake = Mailer(send: to => print("to ${to}"))
fake.send("ada@example.com")
```

- Methods go inside the type: `fn describe(self) -> String { ... }`. A method
  that changes the value is `mutating fn add(x: Int) { self.items.append(x) }`
  and needs a `var`.
- No classes, inheritance, overloading, operator overloading, tuples or
  default parameter values. Several results make a record. Many options
  make an options record with defaults.
- Generics: `fn first<T>(xs: List<T>) -> T?`. No bounds.
- A `String` is always valid UTF-8: text read from files, streams, the
  network and the environment has invalid bytes replaced with `�`. For
  exact bytes use `Bytes`; `bytes.text()` fails on invalid UTF-8,
  `bytes.text_lossy()` replaces.

## Missing values

```
let email: String? = user.email
let shown = email ?? "(none)"                 // a fallback
if email is some(e) { send(e) }               // unwrap
if email is none { return }                   // below: `email` is a String
if name is none or name.is_empty() { return } // on the right of `or` too
let user = users[id] ?? throw NotFound(id: id)
```

`user?.address?.city` is none if `user` or its `address` is none (a
`String?`); combine with `??`: `u?.name ?? "anonymous"`. It reads and calls
(`u?.greeting()`), but can't change what's inside: unwrap for that.
An optional can't go into text as it is (`"${email}"` is an error: it
would print `none`): write `"${email ?? ""}"`.
`m[key]` gives `V?`; `xs[i]` out of range is a bug (it stops the task).
`counts[w] += 1` is an error (a new key would stop the program): write
`counts[w] = (counts[w] ?? 0) + 1`.
Change a value inside a map or list in place: `m[key].append(x)`,
`accounts[id].balance -= 5`. Reading it out and writing it back
(`if m[k] is some(v) { m[k] = grow(v) }`) copies the whole value, since
two places hold it; `if m.take(k) is some(v) { m[k] = grow(v) }` doesn't.

## Errors

```
fn load(path: String) -> Config {               // any function can fail:
  let text = files.read(path)                   // an error here goes up
  return json.decode<Config>(text)              // to the caller by itself
}

let text = try files.read(path) catch err {     // or handle it here
  if err is NotFound { return none }            // flow typing after `is`
  throw Failure(message: "can't load ${path}", cause: err)
}
```

- `err is NotFound` also looks through causes: it's true when `err` or
  an error in its `cause` chain (`Failure(message:, cause: err)`, or your
  own error type's `cause: Error?` field) is a `NotFound`, and inside the
  `if`, `err` is that `NotFound`.
- Any function can fail, and nothing marks it: a failing call's error goes
  up to the caller by itself, up to `main` (which prints it and exits
  with 1) or the server (a 500). The compiler knows which calls can fail.
  The one exception: an error's `message()` and a type's `to_string()`
  can't fail (the language calls them itself); handle errors inside them.
- `try` is only for handling, and always comes with `catch`:
  `try f() catch err { ... }`. Never `f() catch ...`, never `try f()` alone.
- A `catch` block ends with a value, or with `return`, `throw`, `break` or
  `continue`.
- `try ... catch` covers the whole expression between them:
  `try f().text() catch err { "" }` handles both calls.
- Your own errors: a type with `fn message(self) -> String`, or
  `Failure(message: "...")` from the prelude:

```
type NotFound implements Error {
  id: Int

  fn message(self) -> String {
    return "no ${self.id}"
  }
}
```
- Bugs (`panic("...")`, `assert`, overflow, a bad index) can't be caught.
  A panic prints where it happened (`at util.plumb:12`) and the functions
  that led there (`in load_user`, `in main`); in a server, the request
  gets a 500 and the server goes on.

## Functions

```
fn price(item: Item, count: Int) -> Float { ... }
price(item, count: 3)            // with 3+ parameters, name every argument
                                 // after the first: f(a, b: 1, c: 2)
xs.map(x => x * 2)               // lambdas: x => expr, or x => { ... }
xs.map(x => parse(x))            // a lambda that calls a failing function
                                 // can fail too, and so can this `map`
```

- **A function never changes its arguments.** Return the new value and
  assign it: `items = add_item(items, item: x)`.
- **A result may be left unused**: `emails.try_send(x)` alone is fine.
  But a method that only returns a new version of its value changes
  nothing, so dropping its result is an error: `list.sort()` changes the
  list in place, `list.sorted()` returns a new one (`xs = xs.sorted()`).
  A `let` that is never used, and an import that is
  never used, are errors too; so is changing a `var` that is never read
  afterward (`var first = list[0]; first.qty = 9` changes a copy: write
  `list[0].qty = 9`).
- No `return` inside a lambda: its last line is its value.
- A lambda gets a copy of each value it uses, made when the lambda is
  made. So it can't change them, and a `var` it uses can't change after
  it (an error: the lambda would keep seeing the old value). Put the value
  in a `let` first: `let id = user.id` then `() => save(id)`.
- A function kept in a field is called like a method: `rule.test(x)`. A
  named function type (`type Check = fn(Int) -> Bool`) takes any function
  or lambda of that type.
- A function that never comes back is `-> Never`: it must end in
  `process.exit(...)`, `panic(...)`, `throw` or an endless loop. A call to it
  ends a branch like `return` does:

```
fn usage(msg: String) -> Never {
  eprint("error: ${msg}")
  process.exit(2)
}
let n = if args.length > 0 { args[0].to_int() } else { usage("no count") }
```

## Numbers

- `Int` is 64-bit and `Float` is 64-bit. There are no implicit conversions:
  `n.to_float()`, `f.round()`, `f.floor()`, `f.ceil()`.
- **`/` on two `Int`s is an error.** Write `a.div(b)` (whole numbers) or
  `a.to_float() / b.to_float()`. `div` and `%` round down, toward minus
  infinity, as in Python: `(-7).div(2) == -4`, `(-7) % 2 == 1`, so
  `(-1) % 7 == 6` (Go, Rust, C and JS give `-3` and `-1`: mind it when
  porting code that handles negative numbers).
- Bits: `a.bit_and(b)`, `bit_or`, `bit_xor`, `shift_left`, `shift_right`;
  `0xFF`, `0b1010`, `0o755`, `1_000_000`.
- String to numbers: `text.to_int()`, `text.to_float()`. They are
  strict: `" 42"` (trim first), `"1_000"`, `"nan"`, `"inf"` and `"1e400"` are errors.
- **Money is `Decimal`**, never `Float`: `let price: Decimal = 19.99`;
  `+ -` are exact, `*` is exact up to 18 digits after the point (then it
  rounds half up), and a result past 18 significant digits stops the
  program ("Decimal overflow"); `/` is `a.div(b, places: 2)`; `x.round(2)`;
  `n.to_decimal()` from an Int; `1.50` prints as `1.50` and equals `1.5`.
  JSON carries it exactly as a number; as a SQL parameter it is sent as
  text (`"19.99"`), and `query<T>` reads it back into a `Decimal` field.

## Resources: `with`

Files, connections, processes and anything else with a `close()` must be
opened with `with`. It's closed at the end of the block, even on an error.

```
with f = files.create("out.txt") {
  f.write_text("hello\n")
}

with conn = try postgres.connect(url) catch err {     // handle failure to open
  log.error(err.message())
  return
} {
  ...
}
```

- `let f = files.open(p)` is an error: use `with`.
- A lambda that uses the resource can't be kept after the block (put in
  a variable from outside, passed to `router.get` of an outside router,
  returned): it would find the resource closed.
- A record with a `close(self)` method is a resource too. Its fields may
  take other resources: `Client(conn: net.connect(host, port))`.
- A function may return a new resource (`fn open_db() -> db.Connection`).
  To set it up first and close it only on failure, return the `with`
  variable itself:

  ```
  with c = Client(conn: net.connect(host, port)) {
    login(c)              // an error closes it
    return c              // success gives it to the caller, open
  }
  ```

## Several things at once, and work for later

Code runs one step after another: there is no `spawn`, no tasks, locks
or channels, and no `async`/`await`. What would need them is one call:

```
let sizes = urls.parallel_map(limit: 8, transform: u => fetch(u).length)
let answers = http.send_all([weather_request, news_request])   // at once, in order
```

`parallel_map` runs up to `limit` items at a time (one by one on
wasm-server); its lambda sees copies, so nothing is shared. `send_all`
takes as long as the slowest request. State that requests share goes in
the database, never in memory.

Work after the response (a welcome email) and work on a schedule: a
queue and cron, handled by the router (`plumb doc queue`):

```
router.post("/signup", req => {
  queue.send("emails", req.text())                // stored: the reply doesn't wait
  http.text(201, "welcome")
})
router.queue("emails", job => send_welcome(job.payload))   // an error: retried
router.cron("digest", schedule: "0 9 * * 1-5", handler: job => send_digest())

test "a signup sends an email" {                  // tests run what's queued
  router.handle(signup)
  expect router.run_jobs() == 1
}
```

A failed job comes back after 2, 4, 8 and 16 s, and may arrive twice
(`job.id` stays the same). `queue.send_later(name, payload:, delay:)`
waits first. Write queue names and schedules right in the call: the
server learns them from the code on deploy. Cron is in UTC.

`main` has an 8 MB stack: recursion some 100,000 calls deep is fine;
deeper stops the program with "stack overflow".

**Ctrl-C and `kill`** (SIGINT, SIGTERM) stop the program cleanly: waits
(`sleep`, I/O) throw `Cancelled`, `with` blocks close their resources,
`http.serve` stops, and the exit status is 130. A loop that doesn't wait
checks `process.interrupted()`. A second Ctrl-C exits at once. (A program
started with `&` from a script ignores SIGINT, as the shell asks; SIGTERM
still works.)

## Data in and out: `decode<T>`

```
let user = json.decode<User>(body)              // missing fields: an error
                                                // naming the field
let text = json.encode(user)
                                                // `createdAt` in JSON fills
                                                // `created_at` (case, _ and - ignored);
                                                // a key twice, or `name` and `NAME`: an error
let api_body = json.encode_camel(user)          // keys as createdAt
let out = json.encode_with(user, options: json.EncodeOptions(keys: json.Keys.Kebab, omit_none: true))
                                                // created-at; fields that are none left out
let config = env.decode<Config>()               // CONFIG fields from env
let opts = cli.decode<Options>()                // --flag or -flag (-n for a field `n`),
                                                // --help generated: a comment
                                                // above a field describes it
let sub = cli.decode_from<AddOptions>(args.drop(1)) // subcommands: match on args[0]
let rows = conn.query<User>("select id, name from users where age > ?", [18])
```

JSON of unknown shape: `let v = json.parse(text)` gives a `json.Value`
(`Null`, `Bool`, `Number`, `String`, `Array`, `Object`). A number stays as
written: `if v is json.Value.Number(n) { let id = n.to_int() }`. To
build one: `json.number(0.7)`, `json.integer(512)`.

SQL is always written right in the call, with `?` (SQLite) or `$1`
(Postgres) for values. Building SQL from text is a compile error. Parameters
are plain values: `[name, 36, true]`.

Logging goes to standard error: `log.info("started")` (also `debug`, `warn`,
`error`; `LOG_LEVEL=debug`, `LOG_FORMAT=json`). Fields for every message of
a request or job: `let l = log.with_fields({"request": "${id}"})`, then
`l.info("saved")`.

## HTTP

```
fn get_note(req: http.Request) -> http.Response {
  let id = (req.param("id") ?? "").to_int()
  return http.json(200, load_note(id))
}

var router = http.Router()
router.get("/notes/:id", get_note)
router.files("/static", dir: "public")
http.serve(router, port: 8080)
// or: http.serve_with(router, options: http.ServerOptions(port: 8080,
//   host: "127.0.0.1", max_body: 10_000_000, idle_timeout: time.seconds(30)))
// (slow or idle clients are closed: idle / read / write timeouts, 60 s)

let res = http.get("https://example.com")                   // client
let api = http.send(http.ClientRequest(url: u, method: "POST",
  headers: {"authorization": "Bearer ${key}"}, body: json.encode(x).bytes()))

// several at once: the time of the slowest, not the sum (on wasm-server too)
let answers = http.send_all([http.ClientRequest(url: weather_url), http.ClientRequest(url: news_url)])
let weather = answers[0]                                    // answers in order
let news = answers[1]
```

Routes: `get`, `post`, `put`, `patch`, `delete`, `any` (every method),
`add("PROPFIND", pattern: ..., handler: ...)`; `:name` is one path part,
`*name` the rest. HEAD is answered by GET routes; OPTIONS and 405 (with
`allow`) by the router. Parameters come decoded (`%2F` inside a part stays
in it; `+` in a path is a plus). The query: `req.query("q")` (a repeated
name gives its last value), `req.query_all("tag")`, `req.raw_query`. Also
`req.raw_path`, `req.client_ip`, `http.status_text(404)`. Header names are
lower case; `with_header` sets one value per name in any case.

Middleware runs around every request (the first added runs first; one
that answers without calling `next` skips everything added after it, so
add `log_requests` first):

```
router.use(http.log_requests())
router.use(http.cors(["https://app.example"]))
router.use((req, next) => {
  if req.header("authorization") is none { http.text(401, "log in") } else { next(req) }
})
```

WebSockets: `router.websocket("/chat", ws => chat(ws))`; in `chat`,
`while ws.receive() is some(msg) { ws.send_text(...) }`. Client:
`with ws = http.websocket("wss://...") { ... }`.

Forms as records: `req.decode_form<SignUp>()` (urlencoded or multipart;
an unchecked box is `false`, an empty field is missing, a repeated name
fills a `List`), the query: `req.decode_query<Search>()`. As a Map:
`req.form()`; with files: `req.parts()`. Cookies:
`req.cookie("session")`, `res.with_cookie(http.Cookie(name: "session", value: v))`.

Bodies are `Bytes`: `req.text()`, `res.text()` give them as text. A handler's error
becomes a 500 and a log line. `http.serve` logs "listening on ..." itself.

State shared by all requests goes in the database, passed to the
handlers through lambdas (there are no global variables, and on
wasm-server nothing stays in memory between requests):

```
fn add_todo(req: http.Request, conn: db.Connection) -> http.Response {
  let input = json.decode<NewTodo>(req.text())
  let id = conn.insert("insert into todos (title) values (?)", [input.title])
  return http.json(201, Todo(id: id, title: input.title))
}

fn make_router(conn: db.Connection) -> http.Router {
  var router = http.Router()
  router.post("/todos", req => add_todo(req, conn))
  return router
}

fn main() {
  with conn = db.open("main") {
    http.serve(make_router(conn), port: 8080)
  }
}

test "adding a todo" {                  // handlers are tested without a network
  with conn = db.open(":memory:") {     // a database of the test's own
    conn.execute("create table todos (id integer primary key, title text)", [])
    var req = http.request("POST", "/todos")
    req.body = "{\"title\": \"milk\"}".bytes()
    let res = make_router(conn).handle(req)
    expect res.status == 201
  }
}
```

## Templates (Handlebars style)

```
let views = template.load("views")                 // views/*.html escape values
let html = views.render("menu", data: menu)        // {{name}} {{#each items}}...{{/each}}
                                                   // {{#if x}}...{{else}}...{{/if}} {{> header}}
```

A name missing from the data is an error (with the line), not empty text.

## Tests

```
test "discount never goes below zero" {
  expect apply(Discount.Fixed(50.0), total: 30.0) == 0.0
}
```

`expect` prints both sides on failure. Tests go in the same file and can
see private functions. `plumb test` runs the tests of every file in the
project (`plumb test app.plumb`: one file's).

## Building for wasm-server

An app for wasm-server is described in the project's `plumb.toml`:

```
[app]
name = "notes"
main = "main.plumb"
databases = ["main"]            # its own SQLite databases
schema = "schema.sql"           # `create table if not exists ...`, run on deploy
domains = ["notes.example.com"] # HTTPS certificates come by themselves
outbound = ["api.stripe.com"]   # hosts it may call; left out: any public host

[deploy]
server = "https://my-server.example.com"
```

Several apps on one domain (a site and its API, say) make a project:
they share its domains, databases and secrets, and deploy together.

```
[project]
name = "shop"
domains = ["shop.example.com"]
databases = ["main"]
schema = "schema.sql"

[apps.web]
main = "web/main.plumb"         # no path: everything the others don't take

[apps.api]
main = "api/main.plumb"
path = "/api"                   # /api and below; routes keep it: router.post("/api/orders", ...)
```

The frontend is an app of files, on the same domain as the API (no CORS):

```
[apps.site]
static = "frontend/dist"                    # served as they are, with caching
build = "cd frontend && npm run build"      # run before every deploy
spa = true                                  # /orders/42 gets index.html
```

An app with both `main` and `static` serves its files first and runs its
program for everything else; under a `path`, its files are below that
path (`/admin/style.css` is `style.css`).

- `plumb server add root@HOST --domain example.com` once: installs
  wasm-server on a Debian or Ubuntu server over SSH, with HTTPS, and makes
  it the default for `plumb deploy`. Projects then answer at
  `PROJECT.example.com` (one DNS record, `*.example.com`); without
  `--domain`, at `PROJECT.1-2-3-4.sslip.io` with no DNS at all. On a server
  whose ports 80 and 443 belong to Caddy (homebase's): `--behind-caddy`.
- `plumb dev`: runs it on a local wasm-server as the server will, with
  secrets from `.env`, rebuilt on every change (http://localhost:8080).
- `plumb deploy`: live without downtime (`plumb login <server>` for a
  server someone else set up);
  if any app doesn't start, the deploy is refused and the old version of
  every app stays.
- `plumb secret set KEY=VALUE`: environment variables kept on the server,
  never in the project.
- `plumb logs [APP] [-f]` and `plumb stats [APP]`: the project's apps
  together, or one. `plumb stats --queries`: the costliest SQL statements;
  one slower than 200 ms also gets a log line (with an index hint when it
  reads a whole table).
- A deploy checks every SQL statement of the code against the database
  (after the migrations' `before`): a column that doesn't exist, or a
  `query<T>` without a column for one of `T`'s fields, refuses the deploy
  with the file and line. Name computed columns with `as`: `select count(*)
  as n`.
- Every deploy is a release kept on the server (only changed parts are
  uploaded): `plumb releases` lists them with their git commits, `plumb
  rollback` goes back to the previous one (or `plumb rollback ID`).
- **Migrations** change data along with code: `plumb migration new
  add_tags` makes `migrations/20261013120000_add_tags.plumb`. Each runs
  once on every server, in order:

  ```
  import db

  fn before() {          // before the new version goes live; a failure refuses the deploy
    with conn = db.open("main") {
      conn.transaction(() => {       // all or nothing
        conn.execute("alter table notes add column tag text", [])
      })
    }
  }

  fn after() {           // after it's live, in the background: backfills, moving files
    with conn = db.open("main") {
      conn.execute("update notes set tag = 'old' where tag is null", [])
    }
  }
  ```

  A file has `before`, `after` or both; the new code must work while
  `after` hasn't finished. An `after` may run again if it failed halfway:
  make it safe to repeat. The server snapshots the databases before
  `before` steps. `plumb migrations` shows how they went there; `plumb
  migrate` runs the ones that haven't run on this machine's data
  (`data/main.db`: natively, `db.open("main")` is that file).
- **The data is never locked in.** `plumb db pull` downloads a database
  as a plain SQLite file into `data/` (where the program finds it when it
  runs here), `plumb db dump` gives it as SQL, `plumb db shell` runs
  queries on the server's copy; `plumb files pull` downloads the users'
  files into `storage/`. `plumb export` puts the code, the API read from it
  (`API.md`, `openapi.json`), every SQL statement with its SQLite-only parts
  (`SQL.md`), the data, the files and the secrets' names in one folder, with
  a README on running it elsewhere or rewriting it. `plumb api` and `plumb
  sql` print those descriptions.
- Write SQL that works in Postgres too where it costs nothing: `?` works in
  both (the server numbers them for Postgres), `insert ... on conflict`
  rather than `insert or replace`, `coalesce` rather than `ifnull`; `plumb
  sql` lists what's SQLite-only.

`plumb build --target wasm-server app.plumb` makes the component itself
(with wasi-sdk, which plumb downloads into `~/.plumb` the first time,
unless `WASI_SDK` names one). The host
starts a fresh instance for every request and runs `main`; `http.serve`
answers that one request and returns. So the same file also runs as an
ordinary server:

```
fn main() {
  with conn = db.open("notes") {            // a database granted to the app
    var router = http.Router()
    router.get("/notes", req => http.json(200, conn.query<Note>("select id, text from notes", [])))
    http.serve(router, port: 8080)          // the port is ignored there
  }
}
```

- Files from users (avatars, photos, attachments) go in `storage`, by key,
  never into the database or the static files:

  ```
  storage.put_public("avatars/${id}.png", data: part.data, content_type: part.content_type)
  let link = storage.url("docs/${id}.pdf", expires: time.hours(1))      // private files: a link that expires
  let target = storage.upload_url("videos/${id}.mp4", options: storage.Upload(max_bytes: 500_000_000))
  // the client PUTs a big file to `target` itself; then `storage.info(key)` shows it arrived
  ```

  URLs are paths on the app's domain (`/_files/...`), served by the host.
  `plumb run` and `plumb test` keep the files in a directory (a fresh one
  for every test run).
- `db.open(name)` takes a name from `databases` in `[app]` (or one the
  host grants the app), not a file. SQL, `query<T>`,
  `insert` and `transaction` work as usual.
- Nothing stays in memory between requests: keep state in the database.
- `http.get`, `http.send` and the rest of the client work: the host makes
  the connections (and keeps them). It refuses private and loopback
  addresses, and hosts outside the app's `outbound` list if it has one.
- Not available (a compile error names them): `http.open`,
  `http.download`, `net`, `zlib`, public-key crypto. Several HTTP requests
  at once still work: `http.send_all` (the host makes them). `http.file` and `http.stream` give a 500: the host
  serves the app's static files itself. No processes.
- A panic prints its message and line to the host's log, and the request
  gets a 500.

## Common compiler errors and their fixes

| error | fix |
|---|---|
| `a \`Stream\` must be closed: get it with \`with\`` | `with f = ... { }` |
| `/` on two `Int`s is not allowed | `a.div(b)` or `a.to_float() / b.to_float()` |
| there are no global variables | a fixed value: top-level `let`; shared state: the database |
| a top-level `let` holds a fixed value | compute it in a function or in `main` |
| `?.` can't change the value inside an optional | `var v = x ?? ...`, change `v`, then `x = v` |
| `?.` reaches into an optional value, and this is a `T` | use `.` |
| `catch` needs `try` before the call | `try f(x) catch err { ... }` |
| `throws` isn't needed | remove it: any function can fail |
| `try` goes with `catch` | remove `try` |
| `sorted` returns a new value and changes nothing | `xs.sort()`, or `xs = xs.sorted()` |
| `f` has 3 or more parameters: name every argument after the first | `f(a, b: 1, c: 2)` |
| the SQL must be written right here | put values in the parameter list |
| expected `String`, found `Int` | `"${n}"` or `n.to_string()` |
