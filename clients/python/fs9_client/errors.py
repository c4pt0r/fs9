from __future__ import annotations


class Fs9Error(Exception):
    """Base exception for FS9 client errors."""

    def __init__(self, message: str, status_code: int | None = None) -> None:
        super().__init__(message)
        self.message = message
        self.status_code = status_code


class NotFoundError(Fs9Error):
    """Resource not found."""

    def __init__(self, path: str) -> None:
        super().__init__(f"not found: {path}", 404)
        self.path = path


class PermissionDeniedError(Fs9Error):
    """Permission denied."""

    def __init__(self, message: str) -> None:
        super().__init__(f"permission denied: {message}", 403)


class AlreadyExistsError(Fs9Error):
    """Resource already exists."""

    def __init__(self, path: str) -> None:
        super().__init__(f"already exists: {path}", 409)
        self.path = path


class InvalidArgumentError(Fs9Error):
    """Invalid argument provided."""

    def __init__(self, message: str) -> None:
        super().__init__(f"invalid argument: {message}", 400)


class NotDirectoryError(Fs9Error):
    """Path is not a directory."""

    def __init__(self, path: str) -> None:
        super().__init__(f"not a directory: {path}", 400)
        self.path = path


class IsDirectoryError(Fs9Error):
    """Path is a directory when file expected."""

    def __init__(self, path: str) -> None:
        super().__init__(f"is a directory: {path}", 400)
        self.path = path


class DirectoryNotEmptyError(Fs9Error):
    """Directory is not empty."""

    def __init__(self, path: str) -> None:
        super().__init__(f"directory not empty: {path}", 400)
        self.path = path


class InvalidHandleError(Fs9Error):
    """Invalid file handle."""

    def __init__(self) -> None:
        super().__init__("invalid handle", 400)


class ConnectionError(Fs9Error):
    """Connection to server failed."""

    def __init__(self, message: str) -> None:
        super().__init__(f"connection error: {message}")


class TimeoutError(Fs9Error):
    """Request timed out."""

    def __init__(self) -> None:
        super().__init__("request timed out", 504)


def from_response(status_code: int, message: str) -> Fs9Error:
    """Create appropriate error from HTTP response."""
    if status_code == 404:
        return NotFoundError(message)
    if status_code == 403:
        return PermissionDeniedError(message)
    if status_code == 409:
        return AlreadyExistsError(message)
    if status_code == 504:
        return TimeoutError()
    if status_code == 400:
        if "not a directory" in message.lower():
            return NotDirectoryError(message)
        if "is a directory" in message.lower():
            return IsDirectoryError(message)
        if "not empty" in message.lower():
            return DirectoryNotEmptyError(message)
        if "handle" in message.lower():
            return InvalidHandleError()
        return InvalidArgumentError(message)
    return Fs9Error(message, status_code)
