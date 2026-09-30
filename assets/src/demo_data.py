"""Computes the demo scenes with strato itself; writes demo.json next to this file."""

import json
from pathlib import Path
from strato import Index, Segment

titles = [
    ("Machine Learning", 0.95, [("ML", True)]), ("Machine Vision", 0.55, []), ("Machine Translation", 0.5, [("MT", True)]),
    ("Data Science", 0.9, [("data analytics", False)]), ("Database Design", 0.6, []), ("Data Engineering", 0.7, []),
    ("Distributed Systems", 0.65, []), ("Natural Language Processing", 0.85, [("NLP", True)]), ("Computer Science", 0.8, []),
    ("Cloud Computing", 0.75, []), ("Continuous Integration", 0.6, [("CI", True)]), ("Deep Learning", 0.8, []),
    ("Reinforcement Learning", 0.6, [("RL", True)]), ("Computer Vision", 0.7, [("CV", False)]), ("Search Engines", 0.5, []),
    ("Information Retrieval", 0.55, [("IR", True)]), ("Neural Networks", 0.7, []), ("Software Architecture", 0.65, []),
    ("Project Management", 0.8, []), ("Product Design", 0.6, []), ("Graph Databases", 0.45, []), ("Stream Processing", 0.5, []),
    ("Learning Analytics", 0.3, []), ("Political Science", 0.4, []), ("Materials Science", 0.35, []),
]
docs = [(i + 1, t, w, a) for i, (t, w, a) in enumerate(titles)]
index = Index([Segment.build(docs)])
text = {i: t for i, t, _, _ in docs}
scenes = [("mach", "prefix completion"), ("nlp", "abbreviations"), ("machne lerning", "spelling correction"),
          ("datascience", "word decomposition"), ("science", "infix matches")]
out = []
for q, caption in scenes:
    hits = index.autocomplete(q, 4)
    out.append({"query": q, "caption": caption, "hits": [(text[h.id], h.kind, round(h.score, 2)) for h in hits]})
    print(q, out[-1]["hits"])
json.dump(out, open(Path(__file__).with_name("demo.json"), "w"), indent=1)
