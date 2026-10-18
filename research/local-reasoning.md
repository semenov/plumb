# The language through one test: local reasoning (2026-10-18)

The goal (Vlad): a very simple language for servers and CLI utilities,
readable by anyone without learning it. Both kinds of program turn input
into output step by step, touching the outside world. What they need is
small: values (data in and out), steps (a function, `if`, loops), calls to
the outside world, errors that go up, and state that lives outside the
program (the database, files, the queue). Everything else is the cost of
a particular language.

**The test:** can a line be understood from that line and the signature
of its function? Simplicity is not having to keep invisible things in your
head: an object changed elsewhere, someone else running at the same time,
a value that may secretly be missing, a jump nobody wrote, code that
can't be seen from the call. Usage counts are not the argument here.

## Removed today (they failed the test)

| construct | what the reader had to keep in mind |
|---|---|
| `spawn`, `Task`, `Shared`, `Channel`, `http.start` | someone else running at the same time, locks, deadlocks |
| `try` without `catch` | a marker that read as unfinished code |
| `type UserId = Int` | the line said "alias" and meant "new type" |
| `let _ = f()` | a rule visible only as noise |

## Candidates, by what they cost the reader

### 1. Interfaces (your own) — remove, keep `Error`
At `mailer.send(x)` with `mailer: Mailer` an interface, the code that runs
is somewhere else, chosen at run time. Replacing behavior (a fake mailer
in tests) always hides the code at the call; the language should have one
way to do it. A record holding functions (`type Mailer { send: fn(...) }`)
does the same with what the language already has: records and function
values. Interfaces add four concepts on top (the declaration, `implements`,
interface-typed values, lists of mixed types) and their own rules (an
implementation may declare less, changing through an interface element).
`type NotFound implements Error` stays: errors are the one interface the
language itself needs.

### 2. `with` — close resources by themselves at the end of their block
`with f = files.create(p) { ... }` is visible and local, but it brings
rules a reader can't guess: `let f = files.open(p)` is an error; a lambda
that uses the resource can't outlive the block; to hand a resource over
you return the `with` variable. With reference counting the language
already knows when nothing holds a value: a file can close then, at a
known point (the end of the block that holds it, as in Rust, Swift and
C++). Then `with` and its three rules are gone, and `let f = files.create(p)`
just works. What's lost: the block that shows the lifetime.

### 3. Your own generics — the standard library's only
`fn first<T>(xs: List<T>) -> T?` makes the reader hold a type parameter.
Servers and CLIs are about concrete data; `List<T>`, `Map<K, V>`,
`decode<T>` and `query<T>` are where generics pay off, and they're the
standard library's. Same pattern as tasks: the library has them, user code
doesn't. User code writes the concrete type.

### 4. `mutating fn` on your own types — methods return the new value
"A function never changes its arguments" is the rule a reader can trust;
`inventory.restock(...)` silently breaks it for `self`, and needs `var`
and a keyword. `inventory = inventory.restock(...)` says what happens. The
standard library's own changes (`xs.append(x)`, `m[k] = v`, `xs.sort()`)
stay: everyone reads them.

### 5. `m.take(k) is some(v)` — the compiler's job
A performance idiom whose reason can't be seen from the line. The plain
`if m[k] is some(v) { m[k] = grow(v) }` should be made fast instead.

### 6. Variants of your own enums written bare — qualify them
`let s = Published(at: today)`: from the line alone, `Published` could be
a type, a function or a variant. `Status.Published(at: today)` says what
it is, and it's already how variants from other modules are written
(`json.Value.Null`): one way instead of two.

### 7. `err is NotFound` looking through causes — say it
`is` means "this type" everywhere else, and for errors it also searches
the `cause` chain. Either a name that says it (`err.caused_by(NotFound)`)
or accept the exception; small.

### 8. `-> Never` — a word for it
Type-theory jargon: `-> NoReturn` (Python) or an `exits` mark reads
without learning.

## Considered and kept

- Values (copy on assignment), functions that never change their
  arguments, no globals, no null: the core of local reasoning.
- `throws` on the signature with errors going up by themselves: the jump
  is real but written once, where the reader looks for it.
- `T?`, `??`, `is some(v)`, flow typing after `is none { return }`.
- Records, enums with `match` (exhaustive), `if`/`match` as values.
- Modules used with their prefix (`json.decode`): where a name comes from
  is on the line.
- Named arguments, `and`/`or`/`not`, `${...}`, `0..<n`.
- `decode<T>`, `cli.decode<T>`, SQL written in the call: the boundary with
  the outside world is typed and checked.
- Middleware: code around a handler that the handler doesn't show, but
  it's how every web framework works, and it's in one place (the router).
- `fn to_string(self)` used by `"${x}"`: expected, and only for text.
- Loose JSON key matching (`createdAt` fills `created_at`): convenience at
  the boundary, said in the docs.

## Outcome (2026-10-18, the same day)

- **1. Interfaces: removed.** `interface` in a program is an error that
  shows the record of functions; `Error` stays.
- **2. `with`: kept.** On a closer look it passes the test better than the
  alternative: the block *is* the lifetime, visible on the line. Closing
  at the end of the enclosing block would be a rule the reader must know,
  and it matters exactly where code goes wrong (a file written and read
  back in one block: writes are buffered; a temporary folder). The rule
  about lambdas that outlive a resource stays either way, and "use
  `with`" is an error that names its fix.
- **3. Generics: kept.** The packages need them (`postgres.query<T>`,
  `yaml.decode<T>`, `jwt.verify<T>`), and so do small helpers that wrap
  decoding (`api.decode<T>(req)`); a line that uses one reads fine.
- **4. `mutating fn`: kept.** `stack.push(x)` is read right by everyone,
  only works on a `var`, and the alternative breaks every collection-like
  type for one keyword.
- **5. `m.take(k)`: stays as an ordinary method** (remove and return); the
  advice to use it for speed goes once the compiler makes the plain form
  fast (TODO).
- **6. Variants: done.** Made with the enum's name (`Status.Published(at:
  t)`); bare in patterns, where the enum is known.
- **7. `err is X` through causes: kept.** What a handler means by "is this
  a NotFound" is the cause chain; a second spelling would be a second way.
- **8. `-> Never`: kept.** "returns never" reads as English.
