"""Render results/WORKLOAD/*.json as results/WORKLOAD/results.md: python report.py [--workload W]"""
import argparse, json, os
from common import results_dir

ORDER = ["completr", "tantivy", "typesense", "typesense-buckets", "meilisearch", "meilisearch-popfirst"]
QUALITY_COLS = ["mrr_over_prefixes"] + [f"s@{k}_len{L}" for L in (3, 5, 8) for k in (1, 5, 10)] + \
               ["reached_top1", "keystrokes_top1", "reached_top5", "keystrokes_top5", "chars_saved_top5"]


def mb(b):
    return f"{b / 1e6:.0f} MB" if b is not None else "-"


def secs(s):
    return f"{s * 1000:.1f} ms" if s < 1 else f"{s:.2f} s"


def render(res):
    out = []

    def row(*c):
        out.append("| " + " | ".join(str(x) for x in c) + " |")

    any_r = next(iter(res.values()))
    m = any_r["machine"]
    out.append(f"Measured {any_r['date']} on {m['cpu']}, {m['cores']} cores, {mb(m['memory_bytes'])} RAM, {m['os']}, "
               f"Python {m['python']}.\n")
    out.append("## Indexing, size, memory\n")
    row("engine", "version", "index time", "peak memory while indexing", "on disk", "memory after open or index",
        "memory after queries", "open / restart to first hit")
    row(*["---"] * 8)
    for n, r in res.items():
        b = r["build"]
        if b.get("status", "ok") != "ok":
            row(n, r.get("version", "-"), b["status"], *["-"] * 5)
            continue
        if "memory" in r:
            m0, m1 = r["memory"]["rss_delta_after_open_bytes"], r["memory"]["rss_delta_after_5000_queries_bytes"]
        else:
            m0, m1 = r["rss_after_index_bytes"], r["rss_after_queries_bytes"]
        row(n, r["version"], secs(b["index_s"]), mb(b.get("build_rss_bytes", b.get("peak_rss_bytes"))),
            mb(b["disk_bytes"]), mb(m0), mb(m1), secs(r.get("restart_to_first_hit_s", r.get("open_s"))))
    c = res.get("completr", {})
    if "build_8_threads" in c:
        out.append(f"\ncompletr with build_threads=8: {secs(c['build_8_threads']['index_s'])}.")
    if c.get("build_uuid", {}).get("status") == "ok":
        out.append(f"completr with UUID string ids: {mb(c['build_uuid']['disk_bytes'])} on disk, "
                   f"{secs(c['build_uuid']['index_s'])} to index.")
    out.append("\nIn-process engines: RSS growth of a fresh process after opening, then after 5,000 prefix queries; "
               "peak memory while indexing is the build process's peak RSS above the loaded documents. "
               "Servers: RSS of the server process after indexing, then after all queries; peak memory is the "
               "server's peak RSS while indexing.")

    out.append("\n## Latency, limit 10, single client (ms)\n")
    row("set", "engine", "in-process or round-trip p50", "p90", "p99", "mean", "engine-reported p50", "p99", "mean")
    row(*["---"] * 9)
    done = {n: r for n, r in res.items() if "latency" in r}
    for s in ["prefix", "typo1", "typo2", "multiword"]:
        for n, r in done.items():
            l = r["latency"][s]
            c = l.get("in_process_ms") or l["round_trip_ms"]
            e = l.get("engine_ms")
            eng = [f"{e['p50']}", f"{e['p99']}", f"{e['mean']:.2f}"] if e else ["-", "-", "-"]
            row(f"{s} ({c['n']})", n, f"{c['p50']:.3f}", f"{c['p90']:.3f}", f"{c['p99']:.3f}", f"{c['mean']:.3f}", *eng)

    out.append("\n## Throughput, 8 clients, prefix set\n")
    row("engine", "QPS", "clients")
    row("---", "---", "---")
    for n, r in done.items():
        if "throughput" in r:
            row(n, f"{r['throughput']['qps']:.0f}", r["throughput"]["client_model"])

    out.append("\n## Quality: typing the target title one character at a time\n")
    for sname in ["popular", "uniform"]:
        for kind in ["clean", "typo"]:
            out.append(f"\n### {sname} targets, {kind}\n")
            row("engine", "n", *QUALITY_COLS)
            row(*["---"] * (len(QUALITY_COLS) + 2))
            for n, r in done.items():
                q = r["quality"][sname][kind]
                row(n, q["targets"], *[f"{q[c]:.1f}" if "keystrokes" in c else f"{q[c]:.3f}" for c in QUALITY_COLS])
    return "\n".join(out) + "\n"


def render_scale(scales):
    """Index time, memory and size per engine as the corpus grows."""
    out = ["\n## Scale: nested subsets of growing size\n"]

    def row(*c):
        out.append("| " + " | ".join(str(x) for x in c) + " |")

    row("engine", "documents", "status", "index time", "peak memory while indexing", "on disk",
        "memory after open or index", "warm prefix p50 / p99 (ms)")
    row(*["---"] * 8)
    for n, sc in scales.items():
        for r in sc["sizes"]:
            b = r["build"]
            if b["status"] != "ok":
                row(n, f"{r['size']:,}", b["status"], *["-"] * 5)
                continue
            m = r.get("memory", {})
            lat = m.get("warm_prefix_latency") or r.get("warm_prefix_latency") or {}
            lat = lat.get("in_process_ms") or lat.get("round_trip_ms")
            after = m.get("rss_delta_after_open_bytes", r.get("rss_after_index_bytes"))
            row(n, f"{b.get('docs', r['size']):,}", "ok", secs(b["index_s"]),
                mb(b.get("build_rss_bytes", b.get("peak_rss_bytes"))), mb(b["disk_bytes"]), mb(after),
                f"{lat['p50']:.2f} / {lat['p99']:.2f}" if lat else "-")
    return "\n".join(out) + "\n"


if __name__ == "__main__":
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--workload", default=os.environ.get("BENCH_WORKLOAD", "hn"))
    os.environ["BENCH_WORKLOAD"] = ap.parse_args().workload
    d = results_dir()
    load = lambda name: json.loads((d / name).read_text())
    res = {n: load(f"{n}.json") for n in ORDER if (d / f"{n}.json").exists()}
    scales = {n: load(f"scale-{n}.json") for n in ORDER if (d / f"scale-{n}.json").exists()}
    text = render(res) if res else ""
    if scales:
        text += render_scale(scales)
    (d / "results.md").write_text(text)
    print(d / "results.md")
