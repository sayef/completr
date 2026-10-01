"""Usage: bench.py [--workload W] samples [--check] | build ENGINE | mem ENGINE | run ENGINE [--variant V] [--throughput]
| scale ENGINE [--variant V] --sizes N,N,..."""
import argparse, datetime, gc, json, multiprocessing as mp, os, resource, shutil, subprocess, sys, threading, time
import psutil
import memory
from common import (ROOT, load_docs, load_samples, machine, make_samples, quality, results_dir, samples_path,
                    summarize, work_dir)
from engines import MEMORY_LIMIT_BYTES, TIME_LIMIT_S, LimitExceeded, Watch, make

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


def max_rss():
    return resource.getrusage(resource.RUSAGE_SELF).ru_maxrss * (1 if sys.platform == "darwin" else 1024)


def build(e):
    """Index time and size, with the peak RSS sampled during the build above the RSS of the loaded documents.

    Sampling, not the lifetime peak, so the transient memory of parsing the documents is not counted."""
    docs = load_docs()
    gc.collect()
    pid = os.getpid()
    loaded = memory.used(pid)
    with Watch(lambda: memory.used(pid), lambda: None, interval=0.02) as w:
        stats = e.build(docs)
    peak = max(w.peak, memory.used(pid))
    stats.update(docs=len(docs), input_rss_bytes=loaded, peak_rss_bytes=peak,
                 build_rss_bytes=max(0, peak - loaded))
    return stats


def mem(e):
    """RSS growth of a fresh process from opening the index and running 5,000 prefix queries, then warm latency."""
    prefixes = load_samples()["latency"]["prefix"]
    gc.collect()
    pid = os.getpid()
    base = memory.used(pid)
    t = time.perf_counter()
    e.open()
    open_s = time.perf_counter() - t
    after_open = memory.used(pid)
    for q in prefixes[:5000]:
        e.search(q)
    gc.collect()
    after = memory.used(pid)
    return {"baseline_rss_bytes": base, "rss_delta_after_open_bytes": after_open - base,
            "rss_delta_after_5000_queries_bytes": after - base, "peak_rss_bytes": max_rss(), "open_s": open_s,
            "warm_prefix_latency": latency(e, prefixes[5000:7000])}


def sub(cmd, engine, variant=None):
    """Run a bench.py subcommand in a fresh interpreter, within the time and memory limits, and parse its output.

    A run past a limit, or one that fails, returns `{"status": ...}` instead."""
    args = [sys.executable, str(ROOT / "bench.py"), cmd, engine] + (["--variant", variant] if variant else [])
    proc = subprocess.Popen(args, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    status = None
    with Watch(lambda: memory.used(proc.pid), proc.kill) as w:
        try:
            out, err = proc.communicate(timeout=TIME_LIMIT_S)
        except subprocess.TimeoutExpired:
            proc.kill()
            out, err = proc.communicate()
            status = "timeout"
    if status is None and w.exceeded:
        status = "memory limit"
    if status is None and proc.returncode != 0:
        status = "killed" if proc.returncode < 0 else "failed: " + err.strip().splitlines()[-1][:200]
    return {"status": status} if status else {"status": "ok", **json.loads(out)}


def run(a):
    e = make(a.engine, a.variant)
    samples = load_samples()
    lat, qual = samples["latency"], samples["quality"]
    res = {"engine": a.engine, "variant": a.variant, "date": datetime.date.today().isoformat(), "machine": machine()}
    try:
        if e.in_process:
            if a.engine == "completr":
                res["build_8_threads"] = sub("build", a.engine, "8")
                res["build_uuid"] = sub("build", a.engine, "uuid")
            res["build"] = sub("build", a.engine, a.variant)
            if res["build"]["status"] != "ok":
                return save(res, a)
            res["memory"] = sub("mem", a.engine, a.variant)
            res["open_s"] = e.open()
        else:
            try:
                res["build"] = {"status": "ok", **e.build(load_docs())}
            except LimitExceeded as x:
                res["build"] = {"status": str(x)}
                return save(res, a)
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
    save(res, a)


def save(res, a):
    results_dir().mkdir(parents=True, exist_ok=True)
    tag = a.engine + (f"-{a.variant}" if a.variant else "")
    (results_dir() / f"{tag}.json").write_text(json.dumps(res, indent=1) + "\n")
    print(tag, res["build"].get("status"), json.dumps(res["build"].get("index_s")), file=sys.stderr)


def scale(a):
    """Index, open and query nested subsets of growing size; the index data of each size is deleted after it."""
    tag = a.engine + (f"-{a.variant}" if a.variant else "")
    path = results_dir() / f"scale-{tag}.json"
    res = {"engine": a.engine, "variant": a.variant, "date": datetime.date.today().isoformat(), "machine": machine(),
           "time_limit_s": TIME_LIMIT_S, "memory_limit_bytes": MEMORY_LIMIT_BYTES, "sizes": []}
    prefixes = load_samples()["latency"]["prefix"]
    for size in a.sizes:
        os.environ["BENCH_SIZE"] = str(size)
        e = make(a.engine, a.variant)
        row = {"size": size}
        if e.in_process:
            row["build"] = sub("build", a.engine, a.variant)
            if row["build"]["status"] == "ok":
                row["memory"] = sub("mem", a.engine, a.variant)
        else:
            try:
                docs = load_docs()
                row["build"] = {"status": "ok", "docs": len(docs), **e.build(docs)}
                row["rss_after_index_bytes"] = e.rss()
                for q in prefixes[:2000]:
                    e.search(q)
                row["warm_prefix_latency"] = latency(e, prefixes[2000:4000])
            except LimitExceeded as x:
                row["build"] = {"status": str(x)}
            finally:
                e.close()
        res["sizes"].append(row)
        results_dir().mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(res, indent=1) + "\n")
        shutil.rmtree(work_dir(), ignore_errors=True)
        print(tag, size, row["build"]["status"], row["build"].get("index_s"), file=sys.stderr)
        if row["build"]["status"] != "ok":
            break


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--workload", default=os.environ.get("BENCH_WORKLOAD", "hn"))
    ap.add_argument("cmd", choices=["samples", "build", "mem", "run", "scale"])
    ap.add_argument("engine", nargs="?")
    ap.add_argument("--variant")
    ap.add_argument("--throughput", action="store_true")
    ap.add_argument("--check", action="store_true", help="samples: fail if samples.json differs from a fresh draw")
    ap.add_argument("--sizes", type=lambda v: [int(x.replace("_", "")) for x in v.split(",")], help="scale: subset sizes")
    a = ap.parse_args()
    os.environ["BENCH_WORKLOAD"] = a.workload
    if a.cmd == "samples":
        text = json.dumps(make_samples(load_docs(full=True)), indent=1) + "\n"
        path = samples_path()
        if a.check:
            sys.exit(0 if path.exists() and path.read_text() == text else f"{path.name} does not match the dataset")
        path.write_text(text)
    elif a.cmd == "build":
        print(json.dumps(build(make(a.engine, a.variant))))
    elif a.cmd == "mem":
        print(json.dumps(mem(make(a.engine, a.variant))))
    elif a.cmd == "scale":
        scale(a)
    else:
        run(a)


if __name__ == "__main__":
    main()
