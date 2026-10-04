"""AmazonQAC: Amazon search terms with their popularity, and prefixes users typed: python amazonqac.py

Reads two columns of the AmazonQAC training files (huggingface.co/datasets/amazon/AmazonQAC at a pinned
revision, CDLA-Permissive-2.0) over HTTP, keeping each distinct search term once with its popularity, and
writes .cache/data/qac_terms.jsonl: one `{id, title, score}` per term. The test set's prefixes, each with the
id of the term the user then searched (none when the term never occurs in training), go to
.cache/data/qac_test.jsonl.
"""
import json, sys
import duckdb, requests
from common import CACHE

REVISION = "975cbfac4623ffb054c59631e06e39d0831e47fb"
API = f"https://huggingface.co/api/datasets/amazon/AmazonQAC/tree/{REVISION}"
BASE = f"https://huggingface.co/datasets/amazon/AmazonQAC/resolve/{REVISION}"
TERMS = CACHE / "data" / "qac_terms.jsonl"
TEST = CACHE / "data" / "qac_test.jsonl"


def files(split):
    listing = requests.get(f"{API}/{split}", timeout=60).json()
    return sorted(f"{BASE}/{e['path']}" for e in listing if e["path"].endswith(".parquet"))


def main():
    if TERMS.exists() and TEST.exists():
        print("ok", TERMS, file=sys.stderr)
        return
    con = duckdb.connect()
    con.execute("INSTALL httpfs; LOAD httpfs; SET preserve_insertion_order = false")
    con.execute(f"SET temp_directory = '{CACHE / 'work' / 'duckdb'}'")
    train = files("train")
    # A term's popularity is its count of searches, the same on every row it appears in.
    con.execute(f"""CREATE TABLE terms AS
        SELECT row_number() OVER (ORDER BY term) AS id, term AS title, score FROM (
            SELECT final_search_term AS term, max(popularity) AS score
            FROM read_parquet({train!r}) WHERE final_search_term <> '' GROUP BY 1)""")
    con.execute(f"COPY (SELECT id, title, score FROM terms ORDER BY id) TO '{TERMS}.tmp' (FORMAT JSON)")
    (TERMS.parent / f"{TERMS.name}.tmp").rename(TERMS)
    test = files("test")
    con.execute(f"""COPY (
        SELECT t.prefix, terms.id, t.final_search_term AS term FROM read_parquet({test!r}) t
        LEFT JOIN terms ON terms.title = t.final_search_term ORDER BY t.query_id)
        TO '{TEST}' (FORMAT JSON)""")
    n = con.execute("SELECT count(*) FROM terms").fetchone()[0]
    print(f"ok {TERMS}: {n:,} terms", file=sys.stderr)


if __name__ == "__main__":
    main()
