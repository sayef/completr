"""Render the tables of docs/benchmarks.md from bench/results: python doctables.py [WORKLOAD ...]

Each table goes between `<!-- begin WORKLOAD-NAME -->` and `<!-- end WORKLOAD-NAME -->` in the page."""
import json, pathlib, re, sys

ROOT = pathlib.Path(__file__).resolve().parent
DOC = ROOT.parent / "docs" / "benchmarks.md"
NAMES = [("completr", "completr"), ("completr-segments", None), ("tantivy", "tantivy"), ("typesense", "Typesense"),
         ("typesense-buckets", "Typesense, buckets"), ("meilisearch", "Meilisearch"),
         ("meilisearch-popfirst", "Meilisearch, popfirst")]
SETS = [("prefix", "Prefixes as typed"), ("typo1", "One-edit typos"), ("typo2", "Two-edit typos"),
        ("multiword", "Multi-word")]
QUALITY = [("clean", "mrr_over_prefixes", 3), ("clean", "s@1_len3", 3), ("clean", "s@1_len5", 3),
           ("clean", "s@5_len5", 3), ("clean", "s@1_len8", 3), ("clean", "keystrokes_top5", 1),
           ("typo", "mrr_over_prefixes", 3), ("typo", "s@1_len5", 3), ("typo", "s@1_len8", 3),
           ("typo", "reached_top1", 3)]


def mb(b):
    return f"{b / 1e6:.0f} MB"


def secs(s):
    return f"{s * 1000:.1f} ms" if s < 1 else f"{s:.2f} s"


def short(s):
    return f"{s:.2f} s" if s >= 1 else (f"{s * 1000:.0f} ms" if s >= 0.01 else f"{s * 1000:.1f} ms")


def column(values, fmt, lower=True):
    """Formatted values, the best in bold."""
    best = (min if lower else max)(v for v in values if v is not None)
    return ["-" if v is None else f"**{fmt(v)}**" if fmt(v) == fmt(best) else fmt(v) for v in values]


def memory(r):
    if "memory" in r:
        return r["memory"]["rss_delta_after_open_bytes"], r["memory"]["rss_delta_after_5000_queries_bytes"]
    return r["rss_after_index_bytes"], r["rss_after_queries_bytes"]


def tables(workload):
    d = ROOT / "results" / workload
    res = {k: json.loads((d / f"{k}.json").read_text()) for k, _ in NAMES if (d / f"{k}.json").exists()}
    # An engine that did not finish its build is left out; the text says why.
    res = {k: r for k, r in res.items() if r["build"].get("status") == "ok"}
    names = [(k, label or f"completr, {res[k]['build']['segments']} segments, not compacted")
             for k, label in NAMES if k in res]
    rows = [res[k] for k, _ in names]
    out = {}

    idx = column([r["build"]["index_s"] for r in rows], secs)
    peak = column([r["build"].get("build_rss_bytes", r["build"].get("peak_rss_bytes")) for r in rows], mb)
    disk = column([r["build"]["disk_bytes"] for r in rows], mb)
    m0 = column([memory(r)[0] for r in rows], mb)
    m1 = column([memory(r)[1] for r in rows], mb)
    op = column([r.get("restart_to_first_hit_s", r.get("open_s")) for r in rows], short)
    c = res["completr"]
    lines = ["| Engine | Index time | Peak memory while indexing | On disk | Memory after open or index "
             "| Memory after queries | Open or restart to first hit |", "|---|---|---|---|---|---|---|"]
    lines += [f"| {n} | {idx[i]} | {peak[i]} | {disk[i]} | {m0[i]} | {m1[i]} | {op[i]} |"
              for i, (_, n) in enumerate(names)]
    notes = []
    if c["build"].get("segments_written", 1) > 1:
        notes.append(f"completr's index time covers writing {c['build']['segments_written']} segments "
                     f"({secs(c['build']['segments_written_s'])}) and compacting them into one.")
    if c.get("build_8_threads", {}).get("status") == "ok":
        notes.append(f"With 8 build threads, it takes {secs(c['build_8_threads']['index_s'])}.")
    if c.get("build_uuid", {}).get("status") == "ok":
        notes.append(f"With UUID strings as ids instead of integers, its index is "
                     f"{mb(c['build_uuid']['disk_bytes'])} and takes {secs(c['build_uuid']['index_s'])} to build.")
    if notes:
        lines += ["", " ".join(notes)]
    out["index"] = lines

    lines = ["| Engine | Documents | Index time | Peak memory while indexing | On disk |", "|---|---|---|---|---|"]
    for k, n in names:
        f = d / f"scale-{k}.json"
        if not f.exists():
            continue
        for r in json.loads(f.read_text())["sizes"]:
            b = r["build"]
            if b["status"] != "ok":
                lines.append(f"| {n} | {r['size']:,} | {b['status']} | - | - |")
                continue
            lines.append(f"| {n} | {b.get('docs', r['size']):,} | {secs(b['index_s'])} | "
                         f"{mb(b.get('build_rss_bytes', b.get('peak_rss_bytes')))} | {mb(b['disk_bytes'])} |")
    if len(lines) > 2:
        out["scale"] = lines

    lines = ["| Set | Engine | p50 | p90 | p99 | Engine-reported p50 | Engine-reported p99 |",
             "|---|---|---|---|---|---|---|"]
    for key, label in SETS:
        get = lambda r: r["latency"][key].get("in_process_ms") or r["latency"][key]["round_trip_ms"]
        cols = {p: column([get(r)[p] for r in rows], lambda v: f"{v:.2f}") for p in ("p50", "p90", "p99")}
        for i, (_, n) in enumerate(names):
            e = rows[i]["latency"][key].get("engine_ms")
            eng = [str(e["p50"]), str(e["p99"])] if e else ["-", "-"]
            first = f"{label} ({get(rows[i])['n']:,})" if i == 0 else ""
            lines.append(f"| {first} | {n} | {cols['p50'][i]} | {cols['p90'][i]} | {cols['p99'][i]} | "
                         f"{eng[0]} | {eng[1]} |")
    out["latency"] = lines

    with_tp = [(i, n) for i, (_, n) in enumerate(names) if "throughput" in rows[i]]
    qps = column([rows[i]["throughput"]["qps"] for i, _ in with_tp], lambda v: f"{v:,.0f}", lower=False)
    out["throughput"] = ["| Engine | Queries per second |", "|---|---|"] + \
        [f"| {n} | {qps[j]} |" for j, (_, n) in enumerate(with_tp)]

    labelled = [(n, r["labelled"]) for (k, n), r in zip(names, rows) if "labelled" in r and k != "completr-segments"]
    if labelled:
        cols = {m: column([l[m] for _, l in labelled], lambda v: f"{v:.3f}", lower=False)
                for m in ("s@1", "s@10", "mrr@10")}
        out["labelled"] = ["| Engine | S@1 | S@10 | MRR@10 |", "|---|---|---|---|"] + [
            f"| {n} | {cols['s@1'][i]} | {cols['s@10'][i]} | {cols['mrr@10'][i]} |"
            for i, (n, _) in enumerate(labelled)]
    for target in ("popular", "uniform", "misspelled"):
        if not all(target in r["quality"] for r in rows):
            continue
        lines = ["| Engine | MRR | S@1, 3 chars | S@1, 5 chars | S@5, 5 chars | S@1, 8 chars | Keystrokes to top 5 "
                 "| MRR, typo | S@1, 5 chars, typo | S@1, 8 chars, typo | Reached top 1, typo |", "|" + "---|" * 11]
        # Segments rank exactly like their compaction, so the uncompacted row is left out.
        ranked = [(i, n) for i, (k, n) in enumerate(names) if k != "completr-segments"]
        cols = [column([rows[i]["quality"][target][kind][key] for i, _ in ranked],
                       (lambda p: lambda v: f"{v:.{p}f}")(places), lower=key == "keystrokes_top5")
                for kind, key, places in QUALITY]
        lines += [f"| {n} | " + " | ".join(col[j] for col in cols) + " |" for j, (_, n) in enumerate(ranked)]
        out[f"quality-{target}"] = lines
    return out


def splice(text, workload, name, lines):
    begin, end = f"<!-- begin {workload}-{name} -->", f"<!-- end {workload}-{name} -->"
    pattern = re.compile(re.escape(begin) + ".*?" + re.escape(end), re.S)
    if not pattern.search(text):
        sys.exit(f"{DOC.name} has no {begin}")
    return pattern.sub(lambda _: begin + "\n" + "\n".join(lines) + "\n" + end, text)


if __name__ == "__main__":
    text = DOC.read_text()
    for workload in sys.argv[1:] or ["hn", "wiki"]:
        for name, lines in tables(workload).items():
            text = splice(text, workload, name, lines)
    DOC.write_text(text)
    print(DOC)
