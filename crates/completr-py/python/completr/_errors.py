"""Exceptions raised by completr. All derive from `CompletrError`."""


class CompletrError(Exception):
    """Base class of every completr error."""


class ConflictError(CompletrError):
    """A concurrent commit changed what this transaction depends on."""


class CorruptionError(CompletrError):
    """Stored data failed its checksum or structural validation."""


class NotFoundError(CompletrError, LookupError):
    """A requested object, version or index does not exist."""


class InvalidInputError(CompletrError, ValueError):
    """An argument or document was invalid."""


class StorageError(CompletrError, OSError):
    """The file system or object store failed."""
