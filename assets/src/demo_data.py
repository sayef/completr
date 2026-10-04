"""Computes the demo scenes with completr itself; writes demo.json next to this file."""

import json
from pathlib import Path
from completr import Index

titles = [
    ("Bohemian Rhapsody – Queen", 0.95, [("is this the real life", False)]), ("Killer Queen – Queen", 0.6, []),
    ("Dancing Queen – ABBA", 0.85, []), ("Dancing in the Dark – Bruce Springsteen", 0.7, []),
    ("Dancing On My Own – Robyn", 0.5, []), ("Under the Bridge – Red Hot Chili Peppers", 0.75, [("RHCP", True)]),
    ("Smells Like Teen Spirit – Nirvana", 0.8, [("SLTS", True)]), ("Billie Jean – Michael Jackson", 0.9, [("MJ", True)]),
    ("Hotel California – Eagles", 0.8, []), ("Wish You Were Here – Pink Floyd", 0.7, []),
    ("Blinding Lights – The Weeknd", 0.9, []), ("Shape of You – Ed Sheeran", 0.85, []),
    ("Rolling in the Deep – Adele", 0.8, []), ("Bad Guy – Billie Eilish", 0.75, []),
    ("Like a Rolling Stone – Bob Dylan", 0.65, []), ("Sweet Child O' Mine – Guns N' Roses", 0.8, [("GNR", True)]),
    ("Purple Rain – Prince", 0.65, []), ("Take On Me – a-ha", 0.7, []), ("Wonderwall – Oasis", 0.75, []),
    ("Superstition – Stevie Wonder", 0.6, []), ("Africa – Toto", 0.65, []), ("Hey Jude – The Beatles", 0.8, []),
    ("Imagine – John Lennon", 0.75, []),
]
docs = [
    {
        "id": i + 1,
        "text": t,
        "popularity": w,
        "abbreviations": [a for a, is_abbreviation in aliases if is_abbreviation],
        "synonyms": [a for a, is_abbreviation in aliases if not is_abbreviation],
    }
    for i, (t, w, aliases) in enumerate(titles)
]
index = Index.from_documents(docs)
scenes = [("danc", "prefix completion"), ("rhcp", "abbreviations"), ("bohemain rapsody", "spelling correction"),
          ("dancingqueen", "word decomposition"), ("queen", "infix matches")]
out = []
for q, caption in scenes:
    hits = index.complete(q, 3)
    out.append({"query": q, "caption": caption, "hits": [(h.text, h.kind, round(h.score, 2)) for h in hits]})
    print(q, out[-1]["hits"])
json.dump(out, open(Path(__file__).with_name("demo.json"), "w"), indent=1)
