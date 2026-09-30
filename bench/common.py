"""Paths, data loading, the fixed-seed query samples and the metrics."""
import json, math, os, pathlib, platform, random, re, subprocess

ROOT = pathlib.Path(__file__).resolve().parent
CACHE = ROOT / ".cache"
DATA = CACHE / "data" / "hn_stories.jsonl"
BIN = CACHE / "bin"
WORK = CACHE / "work"
RESULTS = ROOT / "results"
SAMPLES = ROOT / "samples.json"
LIMIT = 10
SEED = 20260930


def load_docs():
    """Stories deduplicated by lowercased title, keeping the highest-scored copy, sorted by id."""
    best = {}
    for line in DATA.open():
        d = json.loads(line)
        d["score"] = max(d["score"], 0)
        key = d["title"].lower()
        if key not in best or d["score"] > best[key]["score"]:
            best[key] = d
    return sorted(best.values(), key=lambda d: d["id"])


def weight(score, max_score):
    """Popularity in [0, 1] for strato: log-scaled points."""
    return math.log1p(score) / math.log1p(max_score)


def machine():
    """CPU model, core count, memory and OS of this host."""
    cpu, mem = platform.processor() or platform.machine(), None
    try:
        if platform.system() == "Darwin":
            cpu = subprocess.run(["sysctl", "-n", "machdep.cpu.brand_string"], capture_output=True, text=True).stdout.strip()
            mem = int(subprocess.run(["sysctl", "-n", "hw.memsize"], capture_output=True, text=True).stdout)
            os_name = "macOS " + platform.mac_ver()[0]
        else:
            cpu = next(l.split(":", 1)[1].strip() for l in open("/proc/cpuinfo") if l.startswith("model name"))
            mem = int(next(l.split()[1] for l in open("/proc/meminfo") if l.startswith("MemTotal"))) * 1024
            os_name = platform.platform()
    except (OSError, StopIteration, ValueError):
        os_name = platform.platform()
    return {"cpu": cpu, "cores": os.cpu_count(), "memory_bytes": mem, "os": os_name, "python": platform.python_version()}


WORD = re.compile(r"[A-Za-z]+")
ALPHA = "abcdefghijklmnopqrstuvwxyz"


def edit(word, rng):
    """One random edit (substitute, delete, insert, transpose), never on the first character."""
    i = rng.randrange(1, len(word))
    op = rng.choice("sdit")
    if op == "s":
        return word[:i] + rng.choice(ALPHA.replace(word[i].lower(), "")) + word[i + 1:]
    if op == "d":
        return word[:i] + word[i + 1:]
    if op == "i":
        return word[:i] + rng.choice(ALPHA) + word[i:]
    if i == len(word) - 1:
        i -= 1
    if word[i] == word[i + 1] or i == 0:
        return word[:i] + rng.choice(ALPHA.replace(word[i].lower(), "")) + word[i + 1:]
    return word[:i] + word[i + 1] + word[i] + word[i + 2:]


def typo_query(title, edits, rng):
    """First 1-3 words of the title with `edits` edits in words of >= 5 letters (>= 9 for two in one word)."""
    words = title.split()[: rng.randint(1, 3)]
    idx = [i for i, w in enumerate(words) if WORD.fullmatch(w) and len(w) >= 5]
    if not idx:
        return None
    if edits == 1:
        j = rng.choice(idx)
        words[j] = edit(words[j], rng)
    elif len(idx) >= 2:
        for j in rng.sample(idx, 2):
            words[j] = edit(words[j], rng)
    else:
        j = idx[0]
        if len(words[j]) < 9:
            return None
        words[j] = edit(edit(words[j], rng), rng)
    return " ".join(words)


def make_samples(docs):
    """Latency query sets and quality targets, drawn with a fixed seed."""
    rng = random.Random(SEED)
    titles = [d["title"] for d in docs]
    ids = [d["id"] for d in docs]
    uniform = rng.sample(range(len(docs)), 500)
    prefixes = [t[:k] for t in (titles[i] for i in uniform) for k in range(1, len(t) + 1)]
    typo1, typo2 = [], []
    for pool, n_edits in ((typo1, 1), (typo2, 2)):
        while len(pool) < 1000:
            q = typo_query(titles[rng.randrange(len(docs))], n_edits, rng)
            if q:
                pool.append(q)
    multi = []
    while len(multi) < 2000:
        w = titles[rng.randrange(len(docs))].split()
        if len(w) >= 3:
            k = rng.randint(2, min(4, len(w)))
            s = rng.randrange(0, len(w) - k + 1)
            multi.append(" ".join(w[s:s + k]))
    scores = [d["score"] for d in docs]
    popular, seen = [], set()
    while len(popular) < 500:
        i = rng.choices(range(len(docs)), weights=scores)[0]
        if i not in seen:
            seen.add(i)
            popular.append(i)
    uni_targets = rng.sample(range(len(docs)), 300)

    def target(i):
        words = titles[i].split()
        typo = None
        for j, w in enumerate(words):
            if WORD.fullmatch(w) and len(w) >= 5:
                typo = " ".join(words[:j] + [edit(w, rng)] + words[j + 1:])
                break
        return {"id": ids[i], "title": titles[i], "typo_title": typo}

    return {
        "seed": SEED,
        "latency": {"prefix": prefixes, "typo1": typo1, "typo2": typo2, "multiword": multi},
        "quality": {"popular": [target(i) for i in popular], "uniform": [target(i) for i in uni_targets]},
    }


def load_samples():
    return json.loads(SAMPLES.read_text())


def pct(xs, p):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(round(p / 100 * (len(xs) - 1))))]


def summarize(ms):
    return {"n": len(ms), "p50": pct(ms, 50), "p90": pct(ms, 90), "p99": pct(ms, 99), "mean": sum(ms) / len(ms), "max": max(ms)}


def quality(ranks_by_target):
    """Metrics from (title_len, [rank or None for each prefix length 1..len]) per target."""
    out = {"targets": len(ranks_by_target)}
    rr = [sum(1 / r for r in ranks if r) / len(ranks) for _, ranks in ranks_by_target]
    out["mrr_over_prefixes"] = sum(rr) / len(rr)
    for L in (3, 5, 8):
        for k in (1, 5, 10):
            hits = [ranks[L - 1] is not None and ranks[L - 1] <= k for n, ranks in ranks_by_target if n >= L]
            out[f"s@{k}_len{L}"] = sum(hits) / len(hits)
    for k in (1, 5):
        ks = [next((i + 1 for i, r in enumerate(ranks) if r and r <= k), None) for _, ranks in ranks_by_target]
        reached = [x for x in ks if x]
        out[f"reached_top{k}"] = len(reached) / len(ks)
        out[f"keystrokes_top{k}"] = sum(reached) / max(1, len(reached))
        out[f"chars_saved_top{k}"] = sum(1 - x / n for x, (n, _) in zip(ks, ranks_by_target) if x) / len(ks)
    return out
