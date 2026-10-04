"""English Wikipedia article titles with a month of pageviews: python wiki.py

Downloads the page table of the 2026-09-01 dump and the August 2026 pageviews into .cache/data/wiki/, checking
their checksums, and writes .cache/data/wiki_titles.jsonl: one `{id, title, score}` per article that is not a
redirect, with its page id and its views by users over the month (zero if none).
"""
import gzip, hashlib, json, re, subprocess, sys
import requests
from common import CACHE

DIR = CACHE / "data" / "wiki"
OUT = CACHE / "data" / "wiki_titles.jsonl"
PAGE = ("https://dumps.wikimedia.org/enwiki/20260901/enwiki-20260901-page.sql.gz",
        "sha1", "dd0a15d52efb53a1e7b2e8edb2dfc874a8e0aeae")
# Wikimedia publishes no checksum for pageview dumps; this is the SHA-256 of the file as downloaded on 2026-10-01.
VIEWS = ("https://dumps.wikimedia.org/other/pageview_complete/monthly/2026/2026-08/pageviews-202608-user.bz2",
         "sha256", "8e81aaf18d6b49d919979805a40dff46e95f3cf13e6c657429bd6630e124f95b")

# (page_id, page_namespace, 'page_title', page_is_redirect, ...
ROW = re.compile(rb"\((\d+),(-?\d+),'((?:[^'\\]|\\.)*)',([01]),")
ESCAPE = re.compile(rb"\\(.)")


def fetch(url, algo, digest, directory=DIR):
    """Download url into `directory` unless present, checking its digest."""
    directory.mkdir(parents=True, exist_ok=True)
    path = directory / url.rsplit("/", 1)[1]
    if not path.exists():
        tmp = path.with_suffix(".part")
        with requests.get(url, stream=True, timeout=120) as r, tmp.open("wb") as out:
            r.raise_for_status()
            for chunk in r.iter_content(1 << 22):
                out.write(chunk)
        tmp.rename(path)
    h = hashlib.new(algo)
    with path.open("rb") as f:
        while chunk := f.read(1 << 24):
            h.update(chunk)
    if digest and h.hexdigest() != digest:
        path.unlink()
        sys.exit(f"checksum mismatch for {url}: {h.hexdigest()}")
    print(f"{path.name}: {algo} {h.hexdigest()}", file=sys.stderr)
    return path


def articles(path):
    """Page id by title of every main-namespace page that is not a redirect."""
    found = {}
    with gzip.open(path, "rb") as f:
        # Rows come one per line after each INSERT, or several on the INSERT line in older dumps.
        for line in f:
            if not line.startswith((b"(", b"INSERT INTO `page`")):
                continue
            for page_id, ns, title, redirect in ROW.findall(line):
                if ns == b"0" and redirect == b"0":
                    found[ESCAPE.sub(rb"\1", title).decode("utf-8", "replace")] = int(page_id)
    return found


def views(path, titles):
    """Monthly user views per title in `titles`, summed over access methods."""
    totals = {}
    prefix = b"en.wikipedia "
    seen = False
    # bzcat decompresses at C speed; rows are grouped by wiki, so reading stops after English Wikipedia.
    with subprocess.Popen(["bzcat", str(path)], stdout=subprocess.PIPE, bufsize=1 << 24) as p:
        for line in p.stdout:
            if not line.startswith(prefix):
                if seen:
                    p.kill()
                    break
                continue
            seen = True
            parts = line.split(b" ", 5)
            title = parts[1].decode("utf-8", "replace")
            if title in titles:
                totals[title] = totals.get(title, 0) + int(parts[4])
    return totals


def main():
    if OUT.exists():
        print("ok", OUT, file=sys.stderr)
        return
    page = fetch(*PAGE)
    pageviews = fetch(*VIEWS)
    titles = articles(page)
    print(f"{len(titles):,} articles", file=sys.stderr)
    counts = views(pageviews, titles)
    print(f"{len(counts):,} articles with views", file=sys.stderr)
    tmp = OUT.with_suffix(".tmp")
    with tmp.open("w") as out:
        for title, page_id in titles.items():
            out.write(json.dumps({"id": page_id, "title": title.replace("_", " "), "score": counts.get(title, 0)}) + "\n")
    tmp.rename(OUT)
    print("ok", OUT, file=sys.stderr)


if __name__ == "__main__":
    main()
