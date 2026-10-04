"""Targets typed with real misspellings: python misspellings.py [WORKLOAD ...]

Wikipedia's list of common English misspellings (Wikipedia:Lists of common misspellings/For machines,
revision 1199637275, CC BY-SA 4.0) maps each misspelling to its corrections. For each workload, 500 titles
holding one of those corrections are drawn in proportion to their popularity, with a fixed seed, and the
first such word is replaced by one of its misspellings; the targets go to samples-WORKLOAD-misspelled.json.
"""
import array, itertools, json, os, random, re, sys
import requests
from common import CACHE, ROOT, SEED, load_docs

URL = ("https://en.wikipedia.org/w/index.php?title=Wikipedia:Lists_of_common_misspellings/For_machines"
       "&oldid=1199637275&action=raw")
PAIR = re.compile(r"^\s*([a-z']+)->([a-z', ]+)\s*$")
TARGETS = 500


def misspellings():
    """Misspellings by the correct word, from the cached list."""
    path = CACHE / "data" / "misspellings.txt"
    if not path.exists():
        r = requests.get(URL, timeout=60, headers={"User-Agent": "completr-bench (https://github.com/sayef/completr)"})
        r.raise_for_status()
        path.write_text(r.text)
    wrong = {}
    for line in path.read_text().splitlines():
        m = PAIR.match(line)
        if m:
            for right in (r.strip() for r in m.group(2).split(",")):
                if right.isalpha() and len(right) >= 4:
                    wrong.setdefault(right, []).append(m.group(1))
    return {right: sorted(set(ws)) for right, ws in wrong.items()}


def misspell(word, wrong, rng):
    typo = rng.choice(wrong[word.lower()])
    return typo[0].upper() + typo[1:] if word[0].isupper() else typo


def targets(docs, wrong):
    """Titles drawn by popularity among those with a misspellable word, each with that word misspelt."""
    rng = random.Random(SEED)
    held = [i for i in range(len(docs)) if any(w.lower() in wrong for w in docs.title(i).split())]
    weights = array.array("d", itertools.accumulate(docs.scores[i] + 1 for i in held))
    chosen, seen = [], set()
    while len(chosen) < min(TARGETS, len(held)):
        i = held[rng.choices(range(len(held)), cum_weights=weights)[0]]
        if i in seen:
            continue
        seen.add(i)
        words = docs.title(i).split()
        j = next(k for k, w in enumerate(words) if w.lower() in wrong)
        typo = " ".join(words[:j] + [misspell(words[j], wrong, rng)] + words[j + 1:])
        chosen.append({"id": docs.id(i), "title": docs.title(i), "typo_title": typo})
    return chosen


def main(workloads):
    wrong = misspellings()
    for workload in workloads:
        os.environ["BENCH_WORKLOAD"] = workload
        out = ROOT / f"samples-{workload}-misspelled.json"
        out.write_text(json.dumps(targets(load_docs(full=True), wrong), indent=1, ensure_ascii=False) + "\n")
        print("ok", out, file=sys.stderr)


if __name__ == "__main__":
    main(sys.argv[1:] or ["hn", "wiki", "music", "qac"])
