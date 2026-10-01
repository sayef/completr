"""One adapter per engine: build(docs) -> stats, open(), search(q) -> (ids, engine_ms or None), close()."""
import importlib.metadata as md
import json, os, re, secrets, shutil, subprocess, threading, time, uuid
import psutil, requests
from common import BIN, LIMIT, weight, work_dir

TIME_LIMIT_S = float(os.environ.get("BENCH_TIME_LIMIT_S", 7200))
MEMORY_LIMIT_BYTES = int(float(os.environ.get("BENCH_MEMORY_LIMIT_GB", 12)) * 1e9)


class LimitExceeded(Exception):
    """A build ran past the time or memory limit; the message is the status recorded for it."""


class Watch:
    """Samples `rss()` every 100 ms in the background, keeping the peak; past the memory limit it calls `stop()`."""

    def __init__(self, rss, stop):
        self.rss, self.stop, self.peak, self.exceeded = rss, stop, 0, False
        self.done = threading.Event()
        self.thread = threading.Thread(target=self.run, daemon=True)

    def run(self):
        while not self.done.wait(0.1):
            try:
                self.peak = max(self.peak, self.rss())
            except psutil.Error:
                continue
            if self.peak > MEMORY_LIMIT_BYTES and not self.exceeded:
                self.exceeded = True
                self.stop()

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *exc):
        self.done.set()
        self.thread.join()


def batches(docs, size=250_000):
    for start in range(0, len(docs), size):
        yield docs[start:start + size]


def du(path):
    out = subprocess.run(["du", "-sk", str(path)], capture_output=True, text=True).stdout
    return int(out.split()[0]) * 1024


class Completr:
    """The installed completr package with default Index settings, built through a SegmentWriter.

    Documents stream into the writer, which writes a segment file whenever building more would pass its
    memory budget, as tantivy's writer flushes a segment when its budget fills. An index of several segments
    ranks exactly like one."""
    name = "completr"
    in_process = True
    MEMORY_BUDGET = int(float(os.environ.get("BENCH_COMPLETR_MEMORY_BUDGET_MB", 256)) * (1 << 20))

    def __init__(self, threads=1, uuid_ids=False):
        self.version = "completr " + md.version("completr")
        self.uuid_ids = uuid_ids
        self.dir = work_dir() / ("completr-uuid" if uuid_ids else "completr")
        self.threads = threads

    def paths(self):
        return sorted(self.dir.glob("*.seg"))

    def build(self, docs):
        import completr
        shutil.rmtree(self.dir, ignore_errors=True)
        self.dir.mkdir(parents=True)
        m = max(d["score"] for d in docs)
        doc_id = (lambda d: str(uuid.uuid5(uuid.NAMESPACE_URL, f"doc:{d['id']}"))) if self.uuid_ids else (lambda d: d["id"])
        t = time.perf_counter()
        writer = completr.SegmentWriter(self.dir, memory_budget=self.MEMORY_BUDGET, build_threads=self.threads)
        writer.add({"id": doc_id(d), "text": d["title"], "popularity": weight(d["score"], m)} for d in docs)
        writer.finish()
        build_s = time.perf_counter() - t
        return {"index_s": build_s, "disk_bytes": sum(os.path.getsize(p) for p in self.paths()),
                "segments": len(self.paths()), "memory_budget_bytes": self.MEMORY_BUDGET, "build_threads": self.threads,
                "ids": "uuid strings" if self.uuid_ids else "integers"}

    def open(self):
        import completr
        t = time.perf_counter()
        self.index = completr.Index([completr.Segment.open(p) for p in self.paths()])
        return time.perf_counter() - t

    def search(self, q):
        return [s.id for s in self.index.complete(q, LIMIT)], None

    def close(self):
        self.index = None


TOKEN = re.compile(r"[^\W_]+")


class Tantivy:
    """Autocomplete emulation: complete tokens as terms, the last token as a prefix, fuzzy fallback below 10 hits.

    The title is stored and read with each hit, as a suggestion needs its text."""
    name = "tantivy"
    in_process = True

    def __init__(self):
        self.version = "tantivy-py " + md.version("tantivy")
        self.path = work_dir() / "tantivy"

    def _schema(self):
        import tantivy
        sb = tantivy.SchemaBuilder()
        sb.add_unsigned_field("id", stored=True)
        sb.add_text_field("title", stored=True, tokenizer_name="default")
        sb.add_unsigned_field("score", fast=True)
        return sb.build()

    def build(self, docs):
        import tantivy
        shutil.rmtree(self.path, ignore_errors=True)
        self.path.mkdir(parents=True)
        t = time.perf_counter()
        schema = self._schema()
        index = tantivy.Index(schema, path=str(self.path))
        w = index.writer(heap_size=256_000_000)
        for d in docs:
            w.add_document(tantivy.Document.from_dict({"id": d["id"], "title": d["title"], "score": d["score"]}, schema))
        w.commit()
        w.wait_merging_threads()
        return {"index_s": time.perf_counter() - t, "disk_bytes": du(self.path)}

    def open(self):
        import tantivy
        t = time.perf_counter()
        self.schema = self._schema()
        self.index = tantivy.Index.open(str(self.path))
        self.index.reload()
        self.searcher = self.index.searcher()
        return time.perf_counter() - t

    def _query(self, toks, prefix_last, fuzzy):
        from tantivy import Occur, Query
        subs = []
        for i, tok in enumerate(toks):
            last = prefix_last and i == len(toks) - 1
            dist = (2 if len(tok) >= 9 else 1 if len(tok) >= 5 else 0) if fuzzy else 0
            if dist == 0 and not last:
                q = Query.term_query(self.schema, "title", tok)
            else:
                q = Query.fuzzy_term_query(self.schema, "title", tok, distance=dist, transposition_cost_one=True, prefix=last)
            subs.append((Occur.Must, q))
        return Query.boolean_query(subs)

    def search(self, q):
        toks = [t.lower() for t in TOKEN.findall(q)]
        if not toks:
            return [], None
        prefix_last = not q[-1].isspace()
        ids, seen = [], set()
        for fuzzy in (False, True):
            res = self.searcher.search(self._query(toks, prefix_last, fuzzy), LIMIT, count=False, weight_by_field="score")
            for _, addr in res.hits:
                doc = self.searcher.doc(addr)
                i, _title = doc["id"][0], doc["title"][0]
                if i not in seen:
                    seen.add(i)
                    ids.append(i)
            if len(ids) >= LIMIT:
                break
        return ids[:LIMIT], None

    def close(self):
        self.searcher = self.index = None


class Server:
    """A search server started from bench/.cache/bin and queried over HTTP on localhost."""
    in_process = False
    proc = None

    def __init__(self, variant="default", key=None):
        self.variant = variant
        self.key = key or secrets.token_hex(16)
        self.s = requests.Session()
        self.dir = work_dir() / self.name

    def watch(self):
        """Peak RSS of the server while indexing; past the memory limit the server is killed."""
        return Watch(self.rss, lambda: self.proc.kill())

    def rss(self):
        p = psutil.Process(self.proc.pid)
        return sum(x.memory_info().rss for x in [p] + p.children(recursive=True))

    def wait_ready(self, url):
        for _ in range(600):
            try:
                if self.s.get(url, timeout=1).ok:
                    return
            except requests.RequestException:
                pass
            time.sleep(0.1)
        raise RuntimeError(f"{self.name} did not start")

    def spawn(self, args, env):
        self.proc = subprocess.Popen(args, env=dict(os.environ, **env), cwd=self.dir,
                                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    def open(self):
        return None

    def close(self):
        if self.proc:
            self.proc.terminate()
            try:
                self.proc.wait(20)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait()
            self.proc = None


class Typesense(Server):
    """Typesense with points as default_sorting_field; variant "buckets" sorts by 10 text-match buckets, then points."""
    name = "typesense"
    base = "http://127.0.0.1:8108"

    def __init__(self, variant="default", key=None):
        super().__init__(variant, key)
        self.h = {"X-TYPESENSE-API-KEY": self.key}

    def start(self, fresh=True):
        if fresh:
            shutil.rmtree(self.dir, ignore_errors=True)
        self.dir.mkdir(parents=True, exist_ok=True)
        self.spawn([str(BIN / "typesense-server"), f"--data-dir={self.dir}", "--api-address=127.0.0.1",
                    "--api-port=8108", "--enable-cors=false"], {"TYPESENSE_API_KEY": self.key})
        self.wait_ready(self.base + "/health")
        self.version = "typesense " + self.s.get(self.base + "/debug", headers=self.h).json().get("version", "?")

    def build(self, docs):
        self.start()
        schema = {"name": "hn", "fields": [{"name": "title", "type": "string"}, {"name": "score", "type": "int32"}],
                  "default_sorting_field": "score"}
        self.s.post(self.base + "/collections", headers=self.h, json=schema).raise_for_status()
        t = time.perf_counter()
        with self.watch() as w:
            for batch in batches(docs):
                body = "\n".join(json.dumps({"id": str(d["id"]), "title": d["title"], "score": d["score"]})
                                 for d in batch).encode()
                left = TIME_LIMIT_S - (time.perf_counter() - t)
                try:
                    r = self.s.post(self.base + "/collections/hn/documents/import?action=create&batch_size=10000",
                                    headers=self.h, data=body, timeout=max(left, 1))
                except requests.Timeout:
                    raise LimitExceeded("timeout")
                except requests.ConnectionError:
                    if w.exceeded:
                        raise LimitExceeded("memory limit")
                    raise
                r.raise_for_status()
                bad = [l for l in r.text.splitlines() if '"success":true' not in l]
                assert not bad, bad[:3]
        index_s = time.perf_counter() - t
        n = self.s.get(self.base + "/collections/hn", headers=self.h).json()["num_documents"]
        assert n == len(docs), n
        return {"index_s": index_s, "disk_bytes": du(self.dir), "peak_rss_bytes": w.peak, "schema": schema,
                "search_params": self.params("<q>")}

    def params(self, q):
        p = {"q": q, "query_by": "title", "per_page": LIMIT, "include_fields": "id", "highlight_fields": "none"}
        if self.variant == "buckets":
            p["sort_by"] = "_text_match(buckets: 10):desc,score:desc"
        return p

    def search(self, q):
        r = self.s.get(self.base + "/collections/hn/documents/search", headers=self.h, params=self.params(q)).json()
        return [int(h["document"]["id"]) for h in r["hits"]], r["search_time_ms"]


class Meilisearch(Server):
    """Meilisearch with default ranking rules plus score:desc; variant "popfirst" puts score:desc right after typo."""
    name = "meilisearch"
    base = "http://127.0.0.1:7700"

    def __init__(self, variant="default", key=None):
        super().__init__(variant, key)
        self.h = {"Authorization": f"Bearer {self.key}"}

    def start(self, fresh=True):
        if fresh:
            shutil.rmtree(self.dir, ignore_errors=True)
        self.dir.mkdir(parents=True, exist_ok=True)
        self.spawn([str(BIN / "meilisearch"), "--db-path", str(self.dir / "data.ms"), "--http-addr", "127.0.0.1:7700",
                    "--no-analytics", "--env", "development", "--dump-dir", str(self.dir / "dumps"),
                    "--snapshot-dir", str(self.dir / "snapshots")], {"MEILI_MASTER_KEY": self.key})
        self.wait_ready(self.base + "/health")
        self.version = "meilisearch " + self.s.get(self.base + "/version", headers=self.h).json()["pkgVersion"]

    def wait_task(self, uid, deadline=None):
        while True:
            if deadline and time.perf_counter() > deadline:
                raise LimitExceeded("timeout")
            t = self.s.get(f"{self.base}/tasks/{uid}", headers=self.h).json()
            if t["status"] in ("succeeded", "failed", "canceled"):
                assert t["status"] == "succeeded", t
                return t
            time.sleep(0.05)

    def build(self, docs):
        self.start()
        r = self.s.post(self.base + "/indexes", headers=self.h, json={"uid": "hn", "primaryKey": "id"}).json()
        self.wait_task(r["taskUid"])
        rules = self.s.get(self.base + "/indexes/hn/settings/ranking-rules", headers=self.h).json()
        settings = {"searchableAttributes": ["title"], "displayedAttributes": ["id"], "rankingRules": rules + ["score:desc"]}
        if self.variant == "popfirst":
            settings["rankingRules"] = ["words", "typo", "score:desc"] + [r for r in rules if r not in ("words", "typo")]
        self.wait_task(self.s.patch(self.base + "/indexes/hn/settings", headers=self.h, json=settings).json()["taskUid"])
        t = time.perf_counter()
        with self.watch() as w:
            try:
                # Batches stay under the default 100 MB payload limit; tasks run in the order they were sent.
                uids = []
                for batch in batches(docs):
                    body = "\n".join(json.dumps({"id": d["id"], "title": d["title"], "score": d["score"]})
                                     for d in batch).encode()
                    uids.append(self.s.post(self.base + "/indexes/hn/documents",
                                            headers={**self.h, "Content-Type": "application/x-ndjson"},
                                            data=body).json()["taskUid"])
                for uid in uids:
                    task = self.wait_task(uid, deadline=t + TIME_LIMIT_S)
            except requests.ConnectionError:
                if w.exceeded:
                    raise LimitExceeded("memory limit")
                raise
        index_s = time.perf_counter() - t
        n = self.s.get(self.base + "/indexes/hn/stats", headers=self.h).json()["numberOfDocuments"]
        assert n == len(docs), n
        return {"index_s": index_s, "engine_index_duration": task.get("duration"), "disk_bytes": du(self.dir / "data.ms"),
                "peak_rss_bytes": w.peak, "settings": settings}

    def search(self, q):
        r = self.s.post(self.base + "/indexes/hn/search", headers=self.h,
                        json={"q": q, "limit": LIMIT, "attributesToRetrieve": ["id"]}).json()
        return [h["id"] for h in r["hits"]], r["processingTimeMs"]


def make(name, variant=None, key=None):
    if name == "completr":
        v = variant or ""
        return Completr(threads=int(v) if v.isdigit() else 1, uuid_ids=v == "uuid")
    if name == "tantivy":
        return Tantivy()
    if name == "typesense":
        return Typesense(variant or "default", key)
    if name == "meilisearch":
        return Meilisearch(variant or "default", key)
    raise SystemExit(f"unknown engine {name}")
