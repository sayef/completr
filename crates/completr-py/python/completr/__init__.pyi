from collections.abc import Mapping
from os import PathLike
from typing import Any

from ._async import (
    AsyncClient as AsyncClient,
    AsyncCollection as AsyncCollection,
    AsyncDatabase as AsyncDatabase,
    AsyncEngine as AsyncEngine,
)
from .completr import *  # noqa: F403
from .completr import Database

def connect(
    url: str,
    options: Mapping[str, str] | None = None,
    cache_dir: str | PathLike[str] | None = None,
    *,
    min_word_chars: int = 3,
    max_edit_distance: int = 2,
    fuzzy_prefix_chars: int = 7,
    vector_bits: int = 4,
    compact_keys: bool = False,
    build_threads: int = 1,
) -> Database: ...
