"""A completion API over a completr database: COMPLETR_URL=./completions uvicorn app:app"""

import os
from typing import Annotated

import completr
from fastapi import FastAPI, Query
from pydantic import BaseModel

DATABASE_URL = os.environ.get("COMPLETR_URL", "./completions")
INDEX = os.environ.get("COMPLETR_INDEX", "songs")
CACHE_DIR = os.environ.get("COMPLETR_CACHE_DIR")
SYNC_SECONDS = float(os.environ.get("COMPLETR_SYNC_SECONDS", "5"))

db = completr.connect(DATABASE_URL, cache_dir=CACHE_DIR)
engine = db.engine(sync_every=SYNC_SECONDS)


class Suggestion(BaseModel):
    id: int | str
    text: str
    kind: str
    score: float
    highlights: list[tuple[int, int]]


app = FastAPI(title="completr completions")


@app.get("/complete")
def complete(
    q: Annotated[str, Query(max_length=200)],
    limit: Annotated[int, Query(ge=1, le=50)] = 10,
    contexts: Annotated[list[str] | None, Query()] = None,
) -> list[Suggestion]:
    return [
        Suggestion(id=s.id, text=s.text, kind=s.kind, score=round(s.score, 4), highlights=s.highlights)
        # Empty until the index's first version is written.
        for s in engine.complete(q, [INDEX], limit, contexts=contexts, ignore_missing_layers=True)
    ]


@app.get("/health")
def health() -> dict:
    return {"version": engine.version, "sync": engine.sync_status}
