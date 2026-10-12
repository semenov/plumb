# Plumb on wasm-server: the notes app in C, Rust and Plumb

`pnotes.plumb` is wasm-server's notes app (`guests-c/notes`,
`guests/notes-rs`) in Plumb, built with `plumb build --target
wasm-server`: the same routes and responses, SQLite through
`platform:sdk/sql`. One difference: C and Rust also count views in
`platform:sdk/cache` on every GET (one more host call), and Plumb has no
cache API yet.

`bench.sh` runs all three in wasm-server's Linux benchmark image: 8
workers, 64 connections, 2 s warmup and 8 s per test, oha in the same
container. The server is pinned to CPUs 0-7 and oha to 8-13 (`taskset`),
and the three apps take turns over three rounds; without pinning, runs
on the same machine differed by up to 15%, with it by 1-4%. Measured on
2026-10-13, Apple M3 Max, Docker Desktop VM with 14 vCPUs.

| | C | Rust | Plumb |
|---|---|---|---|
| Lines of app code | 203 | 85 | 34 |
| `.wasm` | 55 KB | 147 KB | 161 KB |
| GET: 20 notes, JSON (median of 3) | 88.5k RPS | 88.9k RPS (100%) | 83.0k RPS (94%) |
| GET, the three rounds | 89.5k, 88.4k, 88.5k | 89.4k, 88.8k, 88.9k | 81.8k, 84.7k, 83.0k |
| POST: SQLite insert (median of 3) | 31.2k RPS | 30.7k RPS (98%) | 29.2k RPS (93%) |
| Instantiations/s, 8 threads | 527k | 569k | 516k |

- **Plumb is about 6% behind C and Rust on GET and 5-7% on POST**, with
  one host call fewer on GET. Its runtime does more per request than the
  hand-written apps: it starts the program (`main`, the router), decodes
  rows into records and encodes JSON generically.
- **Instantiation costs about the same as C** (2% less); Rust is 8-10%
  faster at it.
- **The runtime's first arena chunk was 1 MiB**, grown into the
  instance's memory on every request: 70-71k RPS on GET. A 64 KiB chunk
  on wasm gave 78.6k (+11%) in the same run (before pinning).
- **Plumb takes 2.5× fewer lines than Rust and 6× fewer than C**, and the
  same file runs as a native server.
