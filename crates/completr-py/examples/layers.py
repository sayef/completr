"""Layers: python crates/completr-py/examples/layers.py

One catalogue, many views: tenants, locales, listeners, promotions and takedowns as layers.
"""

from completr import Engine, Index, Segment

catalog = Index.from_documents([
    {"id": "bohemian", "text": "Bohemian Rhapsody – Queen", "popularity": 0.95},
    {"id": "killer", "text": "Killer Queen – Queen", "popularity": 0.6},
    {"id": "dancing", "text": "Dancing Queen – ABBA", "popularity": 0.85},
    {"id": "dark", "text": "Dancing in the Dark – Bruce Springsteen", "popularity": 0.7},
    {"id": "ownown", "text": "Dancing On My Own – Robyn", "popularity": 0.5},
    {"id": "problems", "text": "99 Problems – Jay-Z", "popularity": 0.8},
    {"id": "luftballons", "text": "99 Luftballons – Nena", "popularity": 0.3},
    {"id": "billie", "text": "Billie Jean – Michael Jackson", "popularity": 0.9},
])
engine = Engine()
engine.publish({"catalog": catalog})

def show(query, layers):
    return [(s.text, s.layer) for s in engine.complete(query, layers, limit=4)]

print("\n# tenant")
radio = Index([Segment.build([
    {"id": "bohemian", "text": "Bohemian Rhapsody (Remastered 2011) – Queen", "popularity": 0.95},
    {"id": "session", "text": "Dancing Queen (Live Session) – ABBA", "popularity": 0.8},
], deletes=["killer"])])
engine.publish({"radio": radio})
print(show("queen", ["catalog"]))
print(show("queen", ["catalog", "radio"]))

print("\n# locale")
de = Index.from_documents([
    {"id": "luftballons", "text": "99 Luftballons – Nena", "popularity": 0.98},
    {"id": "atemlos", "text": "Atemlos durch die Nacht – Helene Fischer", "popularity": 0.9},
])
engine.publish({"catalog/de": de})
print(show("99", ["catalog"]))
print(show("99", ["catalog", "catalog/de"]))
print(show("atem", ["catalog"]), show("atem", ["catalog", "catalog/de"]))

print("\n# personal")
user = Index.from_documents([{"id": "ownown", "text": "Dancing On My Own – Robyn", "popularity": 1.0}])
engine.publish({"user/42": user})
print(show("danc", ["catalog"]))
print(show("danc", ["catalog", "user/42"]))

print("\n# promotion and takedown")
promo = Index.from_documents([{"id": "dark", "text": "Dancing in the Dark – Bruce Springsteen", "popularity": 1.0}])
takedown = Index([Segment.build([], deletes=["billie"])])
engine.publish({"promo": promo, "takedowns": takedown})
print(show("danc", ["catalog", "promo"]))
print(show("bill", ["catalog"]), show("bill", ["catalog", "takedowns"]))

print("\n# stacked")
stack = ["catalog", "catalog/de", "radio", "user/42", "takedowns"]
print(show("queen", stack))
print(show("danc", stack))
print(show("99", stack))
