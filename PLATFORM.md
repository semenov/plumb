# Who the platform (Plumb + wasm-server) is for

**The main goal now (2026-10-13):** Plumb compiled to WebAssembly, running
on wasm-server. The native target stays for `plumb run`, `plumb test` and
leaving the platform, but features are designed for wasm first. The state
before this work is the tag `pre-wasm` (commit 9384789, Run 4): Plumb as a
native language for ordinary servers and tools.

## The requirements, as stated (2026-10-13)

- The user is a **solo developer**.
- They make, or vibe-code, **many different projects**.
- **Most of them get almost no traffic.**
- **One project can suddenly take off,** and then it has to be scaled out
  to several servers.

## What follows from them (my reading; to confirm)

1. **Many quiet projects on one server, cheaply.** A project that gets no
   requests should cost next to nothing: no process, no memory held, no
   database server of its own. What matters is density (how many projects
   fit on one small VPS), not the speed of one project.
2. **Starting a project takes minutes:** `plumb.toml`, `plumb dev`,
   `plumb deploy`, with HTTPS and a database by themselves. There is no
   ops work, because a vibe-coder (or their agent) won't do any.
3. **Code that an agent writes must be safe to run next to the other
   projects.** One project's bug, endless loop or memory leak doesn't hurt
   the rest. This is the reason for wasm isolation and per-request limits.
4. **A project that takes off is scaled without rewriting it.** The same
   code, the same `plumb.toml`. It moves from the shared server to servers
   of its own, or gets more of them, by a command or by itself. Its data
   comes along.
5. **The developer sees which project needs attention:** traffic, errors
   and time per project. A project that takes off is noticed before it
   falls over.
6. **No lock-in.** A Plumb app also runs as an ordinary native server, so
   leaving the platform is possible.

## Scaling one project that took off (proposal)

Today the cluster scales the whole server: every project goes to every
worker, and every project's data stays on the primary. For our user, the
unit of scaling has to be the project.

**1. Placement: each project has its own set of servers.** The shared
server holds every project by default. A project can be *moved*
(`plumb scale shop --servers 2..6`, or by the autoscaler) to servers of its
own. Workers are then assigned to a project, and the router sends that
project's requests only to them. The other projects stay where they were
and no longer feel its load.

**2. Its data moves too.** One of the project's servers becomes the owner
of its SQLite databases, and the project's other servers send their SQL
there, the way workers send it to the primary today. The move:
- copy a snapshot of the database while it keeps serving;
- hold writes for a moment (requests wait a second or two, nothing
  fails);
- copy the tail of the WAL;
- switch, and let the writes go on.

For a project that has just taken off, the database is small, so this
takes seconds. SQLite on its own server takes thousands of writes a
second, which is enough for most projects that take off. Postgres is the
step after that, not the first one.

**3. Steps in order, cheapest first:**
- **Vertical:** move the project to one bigger server of its own. Its data
  goes along, and there's no network hop to the database.
- **Horizontal:** add workers that run its code and send SQL to its
  database server.
- **Reads:** keep read replicas of its SQLite on the workers (WAL
  shipping), so reads don't cross the network. Only if needed.

**4. Ingress: one public address for all of it.** The shared server stays
the entry point and proxies a moved project's requests (with HTTPS kept
there). Proxying costs little next to the project's own work. Later, when
the shared server itself gets in the way, switch the project's DNS to its
own servers through the DNS provider's API.

**5. Backups to object storage, continuously.** A project's databases are
streamed to S3-compatible storage (Litestream-style: snapshots plus WAL),
which is needed anyway. The same stream is how data moves to a new server
and how a lost server comes back.

**6. Noticing it in time, and paying only with consent:**
- the platform notices that a project's share of the load keeps growing
  and tells the developer (email or Telegram) with the numbers;
- the autoscaler moves and grows a project only within a monthly budget
  the developer sets in advance;
- with no budget set, it only advises.

## What a solo developer is still missing

The typical setup is either a website, or a mobile or desktop app, with
our backend behind it. What they hit, roughly in order:

1. **Users and sign-in.** Today there is only `crypto.hash_password`.
   Needed:
   - email and password, with password reset;
   - email codes or magic links;
   - Google and Apple sign-in (Apple's is required in an iOS app that has
     other social logins);
   - sessions as cookies for a website, access and refresh tokens for an
     app.

   Every project needs this, and vibe-coded versions of it are the most
   common security hole. It belongs in the platform, as a module the app
   calls.
2. **Files from users:** avatars, photos, attachments. Today there is
   nowhere to keep them (`http.file` is a 500 on wasm). Needed:
   - project-owned storage on disk or S3, with direct uploads by signed
     URL so big files don't pass through the app;
   - image resizing;
   - serving through a CDN.
3. **The frontend next to the API.** A project needs a static site, or a
   single-page app falling back to `index.html`, served on the same domain
   as its API (no CORS). `plumb deploy` should upload it with the code. The
   server can already serve static sites; projects can't use that yet.
4. **Changing the schema of live data.** `create table if not exists`
   covers only the first deploy. Needed:
   - numbered migrations (`migrations/003_add_avatar.sql`), applied once
     and in order, inside the deploy, with a backup taken first;
   - restoring from a backup with one command.
5. **Email:** sign-in codes, receipts, notifications. A `mail.send` backed
   by a provider (Resend, Postmark, SES) or SMTP, kept as a project secret;
   the templates exist already.
6. **Background work and schedules:**
   - sending emails after the response;
   - cleanups, daily digests, retries of webhooks.

   The server has queues and cron; Plumb has no API for them on wasm yet.
7. **AI calls:** a vibe-coded app very often calls an LLM. Today that
   breaks in two ways:
   - the 10-second request limit is too short for such calls;
   - a response can't be streamed to the client (`http.stream` is a 500
     on wasm).

   Needed: streaming responses (SSE), and a longer limit for waiting on
   outgoing HTTP, which costs no CPU.
8. **Typed clients for the frontend.** Plumb knows every request and
   response type, so it can generate the client: TypeScript for a website,
   Swift or Kotlin for an app. The agent writing the frontend then gets
   compile errors instead of guessing JSON. Nobody else in this niche has
   this.
9. **Apps already on people's phones:** old versions keep calling the API
   for months. Needed: a check that a deploy doesn't remove fields or
   routes the previous version had, and a "minimum app version" answer.
10. **Push notifications** to iOS and Android (APNs and FCM) as a
    platform call, plus realtime updates (WebSockets are in the host,
    Plumb's wasm target doesn't reach them yet).
11. **Seeing and fixing data by hand:** a table browser in the dashboard,
    and the database in `plumb dev` too.
12. **Alerts:**
    - errors spiking, a project down, a project taking off, a budget
      running out;
    - sent by email or Telegram, not only shown on a page.
13. **Payments:** Stripe checkout and verified webhooks (`hmac_sha256`
    exists; a small library on top of it).
14. **Abuse:** rate limits per user or IP and protection on sign-up (the
    cache's `incr` is the building block).

Suggested order: 3, 4 and 1 first (a website with users, which almost
every project is), then 2, 5, 6 and 7. After that comes 8, which is what
makes the platform different, with the scaling steps alongside it.

## To think through

- **WebSockets.** The server holds connections and calls the app per
  message (`platform:sdk/websocket`), but Plumb's wasm target doesn't reach
  that interface, and a Plumb server's own `router.websocket` (a task per
  connection) isn't available on wasm. What should a Plumb app write for
  chat or live updates, and what happens to connections on a cluster and
  when a project moves to servers of its own?
- **One app with its own router, or an app per URL?** Today an app is a
  whole program with a router, started fresh for every request. The other
  way is a function per route (as on Vercel or Lambda). To weigh:
  - the cost per request (instantiating a bigger component, running
    `main` and building the router every time);
  - cold starts and memory with many small components;
  - deploy size and time;
  - how an agent writes and tests it;
  - shared code;
  - whether the same file still runs as an ordinary server.

  Measure before deciding.

## Open questions

- At what point does a project outgrow SQLite on its own server, and do we
  then offer Postgres (a dialect the agent has to write for) or SQLite
  shards? It's probably rare; we'll decide when a real project gets there.
- Do we keep the shared server as the only entry point, or move a project
  that took off to its own DNS at once?
- Who sets the budget for extra servers, and where: in `plumb.toml`, or on
  the server only, so an agent can't raise it?
