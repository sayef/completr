"""Render results/*.json as results/results.md."""
import json
from common import RESULTS

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
    row("engine", "version", "index time", "on disk", "memory after open or index", "memory after queries",
        "open / restart to first hit")
    row(*["---"] * 7)
    for n, r in res.items():
        b = r["build"]
        if "memory" in r:
            m0, m1 = r["memory"]["rss_delta_after_open_bytes"], r["memory"]["rss_delta_after_5000_queries_bytes"]
        else:
            m0, m1 = r["rss_after_index_bytes"], r["rss_after_queries_bytes"]
        row(n, r["version"], secs(b["index_s"]), mb(b["disk_bytes"]), mb(m0), mb(m1),
            secs(r.get("restart_to_first_hit_s", r.get("open_s"))))
    if "completr" in res and "build_8_threads" in res["completr"]:
        out.append(f"\ncompletr with build_threads=8: {secs(res['completr']['build_8_threads']['index_s'])}.")
    out.append("\nIn-process engines: RSS growth of a fresh process after opening, then after 5,000 prefix queries. "
               "Servers: RSS of the server process after indexing, then after all queries.")

    out.append("\n## Latency, limit 10, single client (ms)\n")
    row("set", "engine", "in-process or round-trip p50", "p90", "p99", "mean", "engine-reported p50", "p99", "mean")
    row(*["---"] * 9)
    for s in ["prefix", "typo1", "typo2", "multiword"]:
        for n, r in res.items():
            l = r["latency"][s]
            c = l.get("in_process_ms") or l["round_trip_ms"]
            e = l.get("engine_ms")
            eng = [f"{e['p50']}", f"{e['p99']}", f"{e['mean']:.2f}"] if e else ["-", "-", "-"]
            row(f"{s} ({c['n']})", n, f"{c['p50']:.3f}", f"{c['p90']:.3f}", f"{c['p99']:.3f}", f"{c['mean']:.3f}", *eng)

    out.append("\n## Throughput, 8 clients, prefix set\n")
    row("engine", "QPS", "clients")
    row("---", "---", "---")
    for n, r in res.items():
        if "throughput" in r:
            row(n, f"{r['throughput']['qps']:.0f}", r["throughput"]["client_model"])

    out.append("\n## Quality: typing the target title one character at a time\n")
    for sname in ["popular", "uniform"]:
        for kind in ["clean", "typo"]:
            out.append(f"\n### {sname} targets, {kind}\n")
            row("engine", "n", *QUALITY_COLS)
            row(*["---"] * (len(QUALITY_COLS) + 2))
            for n, r in res.items():
                q = r["quality"][sname][kind]
                row(n, q["targets"], *[f"{q[c]:.1f}" if "keystrokes" in c else f"{q[c]:.3f}" for c in QUALITY_COLS])
    return "\n".join(out) + "\n"


if __name__ == "__main__":
    res = {n: json.loads((RESULTS / f"{n}.json").read_text()) for n in ORDER if (RESULTS / f"{n}.json").exists()}
    (RESULTS / "results.md").write_text(render(res))
    print(RESULTS / "results.md")
