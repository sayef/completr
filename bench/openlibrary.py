"""Open Library works with their readers: python openlibrary.py

Downloads the works, authors, reading-log and ratings dumps of 2026-09-30 into .cache/data/openlibrary/,
checking archive.org's SHA-1 sums, and writes .cache/data/books_titles.jsonl: one `{id, title, score}` per
work with a title, its key's number as the id, "title – first author" as the title, and as the score its
reading-log entries plus its ratings (zero if none). The data is CC0.
"""
import json, subprocess, sys
from common import CACHE
from wiki import fetch

DIR = CACHE / "data" / "openlibrary"
OUT = CACHE / "data" / "books_titles.jsonl"
BASE = "https://archive.org/download/ol_dump_2026-09-30/ol_dump_{}_2026-09-30.txt.gz"
SHA1 = {
    "works": "b5492661c5ce6d62aae87e1c14945d22826ee26b",
    "authors": "9e72c055725af370110c5c7def69fc4428b8e4ca",
    "reading-log": "b954190d2d1a0c96d71c62dab993a19f1bb1edb0",
    "ratings": "439f5c0e2c5ceda39749ad478750247747b91283",
}


def lines(path):
    """The dump's lines, decompressed by gzip."""
    with subprocess.Popen(["gzip", "-dc", str(path)], stdout=subprocess.PIPE, bufsize=1 << 24) as p:
        yield from p.stdout


def records(path):
    """The JSON record of each line: type, key, revision, last modified, record."""
    for line in lines(path):
        yield json.loads(line.rsplit(b"\t", 1)[1])


def readers(paths):
    """Reading-log entries and ratings per work key; both dumps start each row with the work key."""
    counts = {}
    for path in paths:
        for line in lines(path):
            key = line.split(b"\t", 1)[0].decode()
            counts[key] = counts.get(key, 0) + 1
    return counts


def main():
    if OUT.exists():
        print("ok", OUT, file=sys.stderr)
        return
    path = {name: fetch(BASE.format(name), "sha1", digest, directory=DIR) for name, digest in SHA1.items()}
    counts = readers([path["reading-log"], path["ratings"]])
    print(f"{len(counts):,} works with readers", file=sys.stderr)
    authors = {}
    for r in records(path["authors"]):
        if isinstance(r.get("name"), str) and r["name"].strip():
            authors[r["key"]] = r["name"].strip()
    print(f"{len(authors):,} authors", file=sys.stderr)
    tmp, n = OUT.with_suffix(".tmp"), 0
    with tmp.open("w") as out:
        for r in records(path["works"]):
            title, key = r.get("title"), r.get("key", "")
            if not isinstance(title, str) or not title.strip() or not key.startswith("/works/OL"):
                continue
            author = next((authors[a["author"]["key"]] for a in r.get("authors", [])
                           if isinstance(a, dict) and isinstance(a.get("author"), dict)
                           and a["author"].get("key") in authors), None)
            title = " ".join(title.split())
            out.write(json.dumps({"id": int(key[len("/works/OL"):-1]), "title": f"{title} – {author}" if author else title,
                                  "score": counts.get(key, 0)}, ensure_ascii=False) + "\n")
            n += 1
    tmp.rename(OUT)
    print(f"ok {OUT}: {n:,} works", file=sys.stderr)


if __name__ == "__main__":
    main()
