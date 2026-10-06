"""Value types shared by the Python API."""

from enum import StrEnum


class MatchKind(StrEnum):
    """How a suggestion matched its query. Members compare equal to their string values."""

    EXACT = "exact"
    PREFIX = "prefix"
    ABBREVIATION = "abbreviation"
    INFIX = "infix"
    FUZZY = "fuzzy"
    SYNONYM = "synonym"
    SEMANTIC = "semantic"

    def __repr__(self) -> str:
        return f"MatchKind.{self.name}"
