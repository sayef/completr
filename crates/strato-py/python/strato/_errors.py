"""Exceptions raised by strato. All derive from `StratoError`."""


class StratoError(Exception):
    """Base class of every strato error."""


class ConflictError(StratoError):
    """A concurrent commit changed what this transaction depends on."""


class CorruptionError(StratoError):
    """Stored data failed its checksum or structural validation."""


class NotFoundError(StratoError, LookupError):
    """A requested object, version or index does not exist."""


class InvalidInputError(StratoError, ValueError):
    """An argument or document was invalid."""


class StorageError(StratoError, OSError):
    """The file system or object store failed."""
