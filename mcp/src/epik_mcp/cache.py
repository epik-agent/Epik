"""Projects V2 ID cache stored in ~/.epik-mcp/cache.json.

Stable IDs (project node ID, field IDs, option IDs) are cached after first
lookup to avoid redundant GraphQL queries. Per-issue item IDs are looked up
on demand and are NOT cached here because issues move between projects.
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

_CACHE_DIR = Path.home() / ".epik-mcp"
_CACHE_FILE = _CACHE_DIR / "cache.json"


def _load() -> dict[str, Any]:
    """Load the cache from disk, returning an empty dict if missing or corrupt."""
    if not _CACHE_FILE.exists():
        return {}
    try:
        return json.loads(_CACHE_FILE.read_text())
    except (json.JSONDecodeError, OSError):
        return {}


def _save(data: dict[str, Any]) -> None:
    """Persist the cache to disk."""
    _CACHE_DIR.mkdir(parents=True, exist_ok=True)
    _CACHE_FILE.write_text(json.dumps(data, indent=2))


def _project_key(project_owner: str, project_number: int) -> str:
    return f"{project_owner}/{project_number}"


def get_project_ids(project_owner: str, project_number: int) -> dict[str, Any] | None:
    """Return cached project IDs or None if not cached.

    The returned dict has the shape::

        {
            "project_id": "<node id>",
            "fields": {
                "<field_name>": {
                    "id": "<field node id>",
                    "options": {"<option_name>": "<option id>", ...}
                },
                ...
            }
        }
    """
    cache = _load()
    return cache.get(_project_key(project_owner, project_number))


def set_project_ids(
    project_owner: str, project_number: int, ids: dict[str, Any]
) -> None:
    """Store project IDs in the cache."""
    cache = _load()
    cache[_project_key(project_owner, project_number)] = ids
    _save(cache)


def invalidate_project(project_owner: str, project_number: int) -> None:
    """Remove a project's entry from the cache."""
    cache = _load()
    key = _project_key(project_owner, project_number)
    cache.pop(key, None)
    _save(cache)


def invalidate_all() -> None:
    """Wipe the entire cache."""
    if _CACHE_FILE.exists():
        _CACHE_FILE.unlink()
