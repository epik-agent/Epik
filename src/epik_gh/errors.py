"""Exception types for epik-gh."""


class EpikGhError(Exception):
    """Base exception for all epik-gh errors."""


class AuthError(EpikGhError):
    """gh is not authenticated. Run `gh auth login` to fix."""

    def __init__(self, message: str = "Not authenticated with GitHub. Run `gh auth login` to authenticate."):
        super().__init__(message)


class NotFoundError(EpikGhError):
    """The requested resource does not exist or the user lacks permission."""


class RateLimitError(EpikGhError):
    """GitHub rate limit exceeded."""

    def __init__(self, message: str = "GitHub rate limit exceeded. Wait and try again."):
        super().__init__(message)


class ValidationError(EpikGhError):
    """Bad arguments were supplied before the call was made."""


class GhError(EpikGhError):
    """gh exited with a non-zero code that doesn't fit a more specific category."""

    def __init__(self, message: str, stderr: str = "", exit_code: int = 1):
        super().__init__(message)
        self.stderr = stderr
        self.exit_code = exit_code
