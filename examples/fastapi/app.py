"""A completion API over a strato database: STRATO_URL=./completions uvicorn app:app"""

import asyncio
import contextlib
import logging
import os
from typing import Annotated

import strato
from fastapi import FastAPI, Query, Request
from pydantic import BaseModel

DATABASE_URL = os.environ.get("STRATO_URL", "./completions")
INDEX = os.environ.get("STRATO_INDEX", "products")
CACHE_DIR = os.environ.get("STRATO_CACHE_DIR")
SYNC_SECONDS = float(os.environ.get("STRATO_SYNC_SECONDS", "5"))

logger = logging.getLogger(__name__)


class Suggestion(BaseModel):
    id: int | str
    text: str
    kind: str
    score: float
    highlights: list[tuple[int, int]]


class Completions(BaseModel):
    version: int | None
    suggestions: list[Suggestion]


async def follow(engine: strato.AsyncEngine) -> None:
    while True:
        await asyncio.sleep(SYNC_SECONDS)
        try:
            await engine.sync()
        except strato.StratoError:
            logger.exception("sync failed; serving the current version")


@contextlib.asynccontextmanager
async def lifespan(app: FastAPI):
    db = await strato.connect_async(DATABASE_URL, cache_dir=CACHE_DIR)
    app.state.engine = await db.engine()
    task = asyncio.create_task(follow(app.state.engine))
    yield
    task.cancel()
    with contextlib.suppress(asyncio.CancelledError):
        await task


app = FastAPI(title="strato completions", lifespan=lifespan)


@app.get("/complete")
async def complete(
    request: Request,
    q: Annotated[str, Query(max_length=200)],
    limit: Annotated[int, Query(ge=1, le=50)] = 10,
    contexts: Annotated[list[str] | None, Query()] = None,
) -> Completions:
    engine: strato.AsyncEngine = request.app.state.engine
    hits = engine.complete(q, [INDEX], limit, contexts=contexts)
    return Completions(
        version=engine.version,
        suggestions=[
            Suggestion(id=s.id, text=s.text, kind=s.kind, score=round(s.score, 4), highlights=s.highlights)
            for s in hits
        ],
    )
