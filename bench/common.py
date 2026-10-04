"""Workloads, paths, data loading, the fixed-seed query samples and the metrics.

The workload (BENCH_WORKLOAD, default hn) and an optional subset size (BENCH_SIZE) come from the environment,
so the subprocesses bench.py starts measure the same data."""
import array, itertools, json, math, os, pathlib, platform, random, re, subprocess, uuid

ROOT = pathlib.Path(__file__).resolve().parent
CACHE = ROOT / ".cache"
DATA = CACHE / "data" / "hn_stories.jsonl"
BIN = CACHE / "bin"
LIMIT = 10
SEED = 20260930


def workload():
    return os.environ.get("BENCH_WORKLOAD", "hn")


def subset_size():
    size = os.environ.get("BENCH_SIZE")
    return int(size) if size else None


def work_dir():
    """Index data of the current workload and subset size."""
    size = subset_size()
    return CACHE / "work" / (workload() + (f"-{size}" if size else ""))


def string_ids():
    """Whether the workload's ids are strings (MBIDs) rather than integers."""
    return workload() == "music"


def results_dir():
    return ROOT / "results" / workload()


def samples_path():
    return ROOT / ("samples.json" if workload() == "hn" else f"samples-{workload()}.json")


def load_hn():
    """Stories deduplicated by lowercased title, keeping the highest-scored copy, sorted by id."""
    best = {}
    for line in DATA.open():
        d = json.loads(line)
        d["score"] = max(d["score"], 0)
        key = d["title"].lower()
        if key not in best or d["score"] > best[key]["score"]:
            best[key] = d
    return sorted(best.values(), key=lambda d: d["id"])


def load_wiki():
    """English Wikipedia articles, not redirects, with a month of user pageviews as their score (see wiki.py)."""
    with (CACHE / "data" / "wiki_titles.jsonl").open() as f:
        yield from (json.loads(line) for line in f)


def load_music():
    """MusicBrainz recordings, "name – artist", with their ListenBrainz listens as their score (see musicbrainz.py)."""
    with (CACHE / "data" / "music_titles.jsonl").open() as f:
        yield from (json.loads(line) for line in f)


def load_qac():
    """Amazon search terms with their number of searches as their score (see amazonqac.py)."""
    with (CACHE / "data" / "qac_terms.jsonl").open() as f:
        yield from (json.loads(line) for line in f)


def load_books():
    """Open Library works, "title – author", with their reading-log entries and ratings as their score."""
    with (CACHE / "data" / "books_titles.jsonl").open() as f:
        yield from (json.loads(line) for line in f)


LOADERS = {"hn": load_hn, "wiki": load_wiki, "music": load_music, "qac": load_qac, "books": load_books}

# Real typed prefixes, each with the id of what the user then searched (None if not in the corpus).
LABELS = {"qac": CACHE / "data" / "qac_test.jsonl"}


def load_labels():
    path = LABELS.get(workload())
    if path is None or not path.exists():
        return None
    with path.open() as f:
        return [json.loads(line) for line in f]


def labelled(search, rows):
    """S@1, S@10 and MRR@10 of each row's target among the results for its prefix."""
    ranks = []
    for row in rows:
        ids = search(row["prefix"])
        ranks.append(ids.index(row["id"]) + 1 if row["id"] is not None and row["id"] in ids else None)
    n = len(ranks)
    return {"queries": n, "in_corpus": sum(r["id"] is not None for r in rows) / n,
            "s@1": sum(r == 1 for r in ranks) / n, "s@10": sum(r is not None for r in ranks) / n,
            "mrr@10": sum(1 / r for r in ranks if r) / n}


class Docs:
    """Documents `{id, title, score}`, with integer scores, held in columns: the titles in one UTF-8 buffer, ids and scores in arrays,
    so tens of millions fit in memory. Ids are integers, or UUID strings held as 16 bytes each."""

    def __init__(self, ids, scores, text, ends):
        self.ids, self.scores, self.text, self.ends = ids, scores, text, ends
        self.uuids = isinstance(ids, (bytes, bytearray))
        self.max_score = max(scores, default=0)

    @classmethod
    def collect(cls, docs):
        ids, scores, text, ends = array.array("q"), array.array("q"), bytearray(), array.array("Q")
        uids = bytearray()
        for d in docs:
            if isinstance(d["id"], str):
                uids += uuid.UUID(d["id"]).bytes
            else:
                ids.append(d["id"])
            scores.append(max(int(d["score"]), 0))
            text += d["title"].encode()
            ends.append(len(text))
        return cls(uids if uids else ids, scores, bytes(text), ends)

    def __len__(self):
        return len(self.scores)

    def id(self, i):
        return str(uuid.UUID(bytes=self.ids[16 * i:16 * i + 16])) if self.uuids else self.ids[i]

    def title(self, i):
        return self.text[self.ends[i - 1] if i else 0:self.ends[i]].decode()

    def __iter__(self):
        for i in range(len(self)):
            yield {"id": self.id(i), "title": self.title(i), "score": self.scores[i]}

    def subset(self, keep):
        """The documents at the ascending positions `keep`."""
        ids = (b"".join(self.ids[16 * i:16 * i + 16] for i in keep) if self.uuids
               else array.array("q", (self.ids[i] for i in keep)))
        text, ends = bytearray(), array.array("Q")
        for i in keep:
            text += self.text[self.ends[i - 1] if i else 0:self.ends[i]]
            ends.append(len(text))
        return Docs(ids, array.array("q", (self.scores[i] for i in keep)), bytes(text), ends)

    def save(self, path):
        path.mkdir(parents=True, exist_ok=True)
        (path / "ids").write_bytes(bytes(self.ids))
        (path / "scores").write_bytes(self.scores.tobytes())
        (path / "text").write_bytes(self.text)
        (path / "ends").write_bytes(self.ends.tobytes())
        (path / "meta.json").write_text(json.dumps({"uuids": self.uuids, "docs": len(self)}))

    @classmethod
    def load(cls, path):
        meta = json.loads((path / "meta.json").read_text())
        cols = []
        for name, code in (("ids", "q"), ("scores", "q"), ("ends", "Q")):
            raw = (path / name).read_bytes()
            if name == "ids" and meta["uuids"]:
                cols.append(raw)
                continue
            a = array.array(code)
            a.frombytes(raw)
            cols.append(a)
        ids, scores, ends = cols
        return cls(ids, scores, (path / "text").read_bytes(), ends)


def load_docs(full=False):
    """The workload's documents; with BENCH_SIZE, a seeded subset, each one nested in the next.

    The loader's output is kept in columns under .cache/data the first time, so later loads read a few files."""
    cols = CACHE / "data" / f"{workload()}.cols"
    if not (cols / "meta.json").exists():
        Docs.collect(LOADERS[workload()]()).save(cols)
    size = None if full else subset_size()
    if size is None:
        return Docs.load(cols)
    # Subsets are drawn once and kept in columns too, so a measured process loads only the subset.
    part = CACHE / "data" / f"{workload()}-{size}.cols"
    if not (part / "meta.json").exists():
        docs = Docs.load(cols)
        if size >= len(docs):
            return docs
        order = list(range(len(docs)))
        random.Random(SEED).shuffle(order)
        docs.subset(sorted(order[:size])).save(part)
    return Docs.load(part)


def weight(score, max_score):
    """Popularity in [0, 1] for completr: log-scaled points."""
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


class _Titles:
    """`docs`' titles by position."""

    def __init__(self, docs):
        self.docs = docs

    def __getitem__(self, i):
        return self.docs.title(i)


def make_samples(docs):
    """Latency query sets and quality targets, drawn with a fixed seed."""
    rng = random.Random(SEED)
    titles = _Titles(docs)
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
    # Cumulative weights computed once draw exactly what `weights=` would, in a fraction of the time.
    cum_scores = array.array("d", itertools.accumulate(docs.scores))
    popular, seen = [], set()
    while len(popular) < 500:
        i = rng.choices(range(len(docs)), cum_weights=cum_scores)[0]
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
        return {"id": docs.id(i), "title": titles[i], "typo_title": typo}

    return {
        "seed": SEED,
        "latency": {"prefix": prefixes, "typo1": typo1, "typo2": typo2, "multiword": multi},
        "quality": {"popular": [target(i) for i in popular], "uniform": [target(i) for i in uni_targets]},
    }


def load_samples():
    return json.loads(samples_path().read_text())


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
