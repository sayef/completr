"""Usage: bench.py samples [--check] | build ENGINE | mem ENGINE | run ENGINE [--variant V] [--throughput]"""
import argparse, datetime, gc, json, multiprocessing as mp, resource, subprocess, sys, threading, time
import psutil
from common import RESULTS, ROOT, SAMPLES, load_docs, load_samples, machine, make_samples, quality, summarize
from engines import make

CLIENTS = 8
TP_SECONDS = 10


def ranks_for(e, text, target_id):
    """Type text one character at a time and record the target's rank after each keystroke."""
    ranks = []
    for k in range(1, len(text) + 1):
        ids, _ = e.search(text[:k])
        ranks.append(ids.index(target_id) + 1 if target_id in ids else None)
    return len(text), ranks


def latency(e, queries):
    rtt, eng = [], []
    for q in queries:
        t = time.perf_counter()
        _, ms = e.search(q)
        rtt.append((time.perf_counter() - t) * 1000)
        if ms is not None:
            eng.append(ms)
    out = {"in_process_ms" if e.in_process else "round_trip_ms": summarize(rtt)}
    if eng:
        out["engine_ms"] = summarize(eng)
    return out


def http_worker(args):
    name, variant, key, queries, offset, deadline = args
    e = make(name, variant, key)
    n = 0
    while time.time() < deadline:
        e.search(queries[(offset + n) % len(queries)])
        n += 1
    return n


def throughput(e, name, variant, queries):
    """Closed-loop QPS: threads for in-process engines, processes over HTTP for servers."""
    if not e.in_process:
        with mp.get_context("spawn").Pool(CLIENTS) as pool:
            deadline = time.time() + TP_SECONDS + 2
            args = [(name, variant, e.key, queries, c * len(queries) // CLIENTS, deadline) for c in range(CLIENTS)]
            t0 = time.time()
            total = sum(pool.map(http_worker, args))
            return {"clients": CLIENTS, "qps": total / (deadline - t0), "client_model": "processes"}
    counts = [0] * CLIENTS
    deadline = time.time() + TP_SECONDS

    def run(c):
        i = c * len(queries) // CLIENTS
        while time.time() < deadline:
            e.search(queries[i % len(queries)])
            i += 1
            counts[c] += 1

    ts = [threading.Thread(target=run, args=(c,)) for c in range(CLIENTS)]
    for t in ts:
        t.start()
    for t in ts:
        t.join()
    return {"clients": CLIENTS, "qps": sum(counts) / TP_SECONDS, "client_model": "threads"}


def mem(e):
    """RSS growth of a fresh process from opening the index and running 5,000 prefix queries."""
    prefixes = load_samples()["latency"]["prefix"][:5000]
    gc.collect()
    proc = psutil.Process()
    base = proc.memory_info().rss
    e.open()
    after_open = proc.memory_info().rss
    for q in prefixes:
        e.search(q)
    gc.collect()
    return {"baseline_rss_bytes": base, "rss_delta_after_open_bytes": after_open - base,
            "rss_delta_after_5000_queries_bytes": proc.memory_info().rss - base,
            "peak_rss_bytes": resource.getrusage(resource.RUSAGE_SELF).ru_maxrss * (1 if sys.platform == "darwin" else 1024)}


def sub(cmd, engine, variant=None):
    """Run a bench.py subcommand in a fresh interpreter and parse its JSON output."""
    args = [sys.executable, str(ROOT / "bench.py"), cmd, engine] + (["--variant", variant] if variant else [])
    return json.loads(subprocess.run(args, capture_output=True, text=True, check=True).stdout)


def run(a):
    e = make(a.engine, a.variant)
    samples = load_samples()
    lat, qual = samples["latency"], samples["quality"]
    res = {"engine": a.engine, "variant": a.variant, "date": datetime.date.today().isoformat(), "machine": machine()}
    try:
        if e.in_process:
            if a.engine == "completr":
                res["build_8_threads"] = sub("build", a.engine, "8")
            res["build"] = sub("build", a.engine, a.variant)
            res["memory"] = sub("mem", a.engine, a.variant)
            res["open_s"] = e.open()
        else:
            res["build"] = e.build(load_docs())
            res["rss_after_index_bytes"] = e.rss()
        res["version"] = e.version
        for q in lat["prefix"][:3000] + lat["typo1"][:200] + lat["multiword"][:200]:
            e.search(q)
        res["latency"] = {k: latency(e, v) for k, v in lat.items()}
        res["quality"] = {}
        for set_name, targets in qual.items():
            clean = [ranks_for(e, t["title"], t["id"]) for t in targets]
            typo = [ranks_for(e, t["typo_title"], t["id"]) for t in targets if t["typo_title"]]
            res["quality"][set_name] = {"clean": quality(clean), "typo": quality(typo)}
        if not e.in_process:
            res["rss_after_queries_bytes"] = e.rss()
        if a.throughput:
            res["throughput"] = throughput(e, a.engine, a.variant, lat["prefix"])
        if not e.in_process:
            e.close()
            t = time.perf_counter()
            e.start(fresh=False)
            while not e.search("google")[0]:
                time.sleep(0.05)
            res["restart_to_first_hit_s"] = time.perf_counter() - t
            time.sleep(1)
            res["rss_after_restart_bytes"] = e.rss()
    finally:
        e.close()
    RESULTS.mkdir(exist_ok=True)
    tag = a.engine + (f"-{a.variant}" if a.variant else "")
    (RESULTS / f"{tag}.json").write_text(json.dumps(res, indent=1) + "\n")
    print(tag, res["version"], json.dumps(res["build"].get("index_s")), file=sys.stderr)


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("cmd", choices=["samples", "build", "mem", "run"])
    ap.add_argument("engine", nargs="?")
    ap.add_argument("--variant")
    ap.add_argument("--throughput", action="store_true")
    ap.add_argument("--check", action="store_true", help="samples: fail if samples.json differs from a fresh draw")
    a = ap.parse_args()
    if a.cmd == "samples":
        text = json.dumps(make_samples(load_docs()), indent=1) + "\n"
        if a.check:
            sys.exit(0 if SAMPLES.exists() and SAMPLES.read_text() == text else "samples.json does not match the dataset")
        SAMPLES.write_text(text)
    elif a.cmd == "build":
        print(json.dumps(make(a.engine, a.variant).build(load_docs())))
    elif a.cmd == "mem":
        print(json.dumps(mem(make(a.engine, a.variant))))
    else:
        run(a)


if __name__ == "__main__":
    main()
