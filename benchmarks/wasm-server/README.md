# Plumb on wasm-server: the notes app in C, Rust and Plumb

`pnotes.plumb` is wasm-server's notes app (`guests-c/notes`,
`guests/notes-rs`) in Plumb, built with `plumb build --target
wasm-server`: the same routes and responses, SQLite through
`platform:sdk/sql`. One difference: C and Rust also count views in
`platform:sdk/cache` on every GET (one more host call), and Plumb has no
cache API yet.

`bench.sh` runs all three in wasm-server's Linux benchmark image: 8
workers, 64 connections, 2 s warmup and 8 s per test, oha in the same
container, two rounds. Measured on 2026-10-13, Apple M3 Max, Docker
Desktop VM with 14 vCPUs.

| | C | Rust | Plumb |
|---|---|---|---|
| Lines of app code | 203 | 85 | 34 |
| `.wasm` | 55 KB | 147 KB | 161 KB |
| GET: 20 notes, JSON, run A | 85.5k–86.5k RPS | — | 78.6k RPS |
| GET, run B | 72.2k–78.3k RPS | 75.4k–80.9k RPS | 68.3k–72.6k RPS |
| POST: SQLite insert, run A | 30.6k–31.7k RPS | — | 28.9k–29.0k RPS |
| POST, run B | 30.3k–30.9k RPS | 30.9k RPS | 28.7k–28.8k RPS |
| Instantiations/s, 8 threads, run B | 430k | 465k | 414k |

Run A was C and Plumb only, run B is `bench.sh`; the machine was busier
during B, so compare columns within a run.

- **Plumb is 7–10% behind C and Rust on GET and 6–7% on POST**, with one
  host call fewer on GET. Its runtime does more per request than the
  hand-written C: it starts the program (`main`, the router), decodes
  rows into records and encodes JSON generically.
- **Instantiation costs the same as C** (about 4% less, within noise);
  Rust is 8–12% faster at it.
- **The runtime's first arena chunk was 1 MiB**, grown into the
  instance's memory on every request: 70–71k RPS on GET. A 64 KiB chunk
  on wasm gave 78.6k (+11%) in the same run.
- **Plumb takes 2.5× fewer lines than Rust and 6× fewer than C**, and the
  same file runs as a native server.
