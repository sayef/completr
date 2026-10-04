"""MusicBrainz recordings with their ListenBrainz listens: python musicbrainz.py

Downloads the MusicBrainz core dump of 2026-09-30 and the ListenBrainz statistics dump of 2026-09-15 into
.cache/data/musicbrainz/, checking their published SHA-256 sums, and writes .cache/data/music_titles.jsonl:
one `{id, title, score}` per recording, with its MBID as the id, "name – artist" as the title, and as the
score the listens counted in ListenBrainz users' all-time top recordings (zero if none).
"""
import json, subprocess, sys, uuid
from common import CACHE
from wiki import fetch

DIR = CACHE / "data" / "musicbrainz"
OUT = CACHE / "data" / "music_titles.jsonl"
MB = ("https://data.metabrainz.org/pub/musicbrainz/data/fullexport/20260930-002222/mbdump.tar.bz2",
      "sha256", "4342c77212a8807e351e619b49162bc796e19419628bad3c9e791dbdd8e5f587")
LB_DUMP = "listenbrainz-statistics-dump-20260915-000002"
LB = (f"https://data.metabrainz.org/pub/musicbrainz/listenbrainz/fullexport/"
      f"listenbrainz-dump-2663-20260915-000002-full/{LB_DUMP}.tar.zst",
      "sha256", "be24cf7773f23c0fe32420ae27429547cd87aba55b5aad76724b216d1565b4e3")


def listens(path):
    """Listens per recording MBID, summed over users' all-time top recordings."""
    member = f"{LB_DUMP}/lbdump/statistics/recordings_all_time.jsonl"
    totals = {}
    cmd = f"zstd -dc '{path}' | tar -xO '{member}'"
    with subprocess.Popen(cmd, shell=True, stdout=subprocess.PIPE, bufsize=1 << 24) as p:
        for line in p.stdout:
            for r in json.loads(line)["data"]:
                mbid = r.get("recording_mbid")
                if mbid:
                    key = uuid.UUID(mbid).bytes
                    totals[key] = totals.get(key, 0) + r["listen_count"]
    return totals


def unescape(field):
    """A PostgreSQL COPY text field."""
    if "\\" not in field:
        return field
    return field.replace("\\\\", "\0").replace("\\t", "\t").replace("\\n", "\n").replace("\\r", "\r") \
        .replace("\0", "\\")


def tables(path):
    """Extracts the recording and artist_credit tables next to the dump, once."""
    names = ["mbdump/recording", "mbdump/artist_credit"]
    if not all((DIR / n).exists() for n in names):
        cmd = f"pbzip2 -dc '{path}' | tar -xf - -C '{DIR}' {' '.join(names)}"
        subprocess.run(cmd, shell=True, check=True)
    return [DIR / n for n in names]


def main():
    if OUT.exists():
        print("ok", OUT, file=sys.stderr)
        return
    DIR.mkdir(parents=True, exist_ok=True)
    recording, artist_credit = tables(fetch(*MB, directory=DIR))
    counts = listens(fetch(*LB, directory=DIR))
    print(f"{len(counts):,} recordings with listens", file=sys.stderr)
    # artist_credit: id, name, ...
    artists = {}
    with artist_credit.open(encoding="utf-8") as f:
        for line in f:
            cols = line.rstrip("\n").split("\t")
            artists[cols[0]] = unescape(cols[1])
    tmp, n = OUT.with_suffix(".tmp"), 0
    # recording: id, gid, name, artist_credit, ...
    with recording.open(encoding="utf-8") as f, tmp.open("w") as out:
        for line in f:
            cols = line.rstrip("\n").split("\t")
            gid, name, credit = cols[1], unescape(cols[2]), artists.get(cols[3], "")
            title = f"{name} – {credit}" if credit else name
            score = counts.get(uuid.UUID(gid).bytes, 0)
            out.write(json.dumps({"id": gid, "title": title, "score": score}, ensure_ascii=False) + "\n")
            n += 1
    tmp.rename(OUT)
    print(f"ok {OUT}: {n:,} recordings", file=sys.stderr)


if __name__ == "__main__":
    main()
