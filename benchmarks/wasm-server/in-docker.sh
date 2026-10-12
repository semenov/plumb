#!/bin/sh
# Inside the image (bench.sh starts it): the C, Rust and Plumb notes
# apps in one wasm-server run: 8 workers, 64 connections, 2 s warmup and
# 8 s per test, oha in the same container. The server runs on CPUs 0-7
# and oha on the rest, so they don't take each other's cores; three
# rounds, alternating the apps, and the medians at the end.
cd /cmp/work
cp /bench/wasm/notes_rs.wasm rs.wasm
for w in c rs pl; do wasm-server compile $w.wasm -o /tmp/$w.cwasm >/dev/null; done
echo "== runtime only (wasm-server bench, 8 threads, 5 s)"
for w in c rs pl; do echo "-- $w"; wasm-server bench /tmp/$w.cwasm --mode instantiate -t 8 2>&1 | tail -4; done
run() { # name route
  rm -rf /tmp/w-$1.db* /tmp/wd-$1
  taskset -c 0-7 wasm-server serve --listen 127.0.0.1:8080 --workers 8 --app $2=/tmp/$1.cwasm --db notes=sqlite:/tmp/w-$1.db --db-init notes=/cmp/work/notes.sql --grant $2=notes --data-dir /tmp/wd-$1 >/tmp/w-$1.log 2>&1 &
  pid=$!; sleep 2
  taskset -c 8-13 oha --no-tui -n 20 -c 1 -m POST -d "a note with \"quotes\" and some text" http://127.0.0.1:8080/$2 >/dev/null
  for kind in GET POST; do
    if [ $kind = GET ]; then args="-c 64"; else args="-c 64 -m POST -d hello"; fi
    taskset -c 8-13 oha --no-tui -z 2s $args http://127.0.0.1:8080/$2 >/dev/null 2>&1
    rss0=$(awk '/VmRSS/{print $2}' /proc/$pid/status)
    taskset -c 8-13 oha --no-tui --output-format json -z 8s $args http://127.0.0.1:8080/$2 2>/dev/null | python3 -c '
import json,sys
d=json.load(sys.stdin); s=d["summary"]; p=d["latencyPercentiles"]; c=d["statusCodeDistribution"]
print("  %-3s %-4s %8.0f rps  p50 %.2f ms  p99 %.2f ms  codes %s  rss %s KiB" % (sys.argv[1], sys.argv[2], s["requestsPerSec"], p["p50"]*1000, p["p99"]*1000, dict(c), sys.argv[3]))' $1 $kind $rss0 | tee -a /tmp/all.txt
  done
  kill $pid; wait $pid 2>/dev/null
}
for round in 1 2 3; do
  echo "== round $round"
  case $round in 1) order="c rs pl" ;; 2) order="rs pl c" ;; *) order="pl c rs" ;; esac
  for w in $order; do
    case $w in c) run c cnotes ;; rs) run rs rnotes ;; pl) run pl pnotes ;; esac
  done
done
echo "== medians of the rounds"
python3 -c '
import statistics, collections
r = collections.defaultdict(list)
for line in open("/tmp/all.txt"):
    f = line.split()
    r[(f[0], f[1])].append(float(f[2]))
for kind in ("GET", "POST"):
    c = statistics.median(r[("c", kind)])
    for app in ("c", "rs", "pl"):
        v = r[(app, kind)]
        print("  %-3s %-4s median %7.0f rps  (%s)  %3.0f%% of C" % (app, kind, statistics.median(v), " ".join("%.0f" % x for x in v), 100 * statistics.median(v) / c))'
