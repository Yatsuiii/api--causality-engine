"""History routes — list, view, and manage execution history."""

from fastapi import APIRouter, HTTPException

from services.storage import (
    clear_history,
    delete_history_entry,
    get_history_entry,
    list_history,
)

router = APIRouter()


@router.get("")
def list_entries(limit: int = 50):
    """List past execution entries, newest first."""
    return [e.model_dump() for e in list_history(limit=limit)]


@router.get("/{entry_id}")
def get_entry(entry_id: str):
    """Get full execution details for a history entry."""
    entry = get_history_entry(entry_id)
    if entry is None:
        raise HTTPException(404, f"History entry '{entry_id}' not found")
    return entry.model_dump()


@router.delete("/{entry_id}")
def delete_entry(entry_id: str):
    """Delete a single history entry."""
    if not delete_history_entry(entry_id):
        raise HTTPException(404, f"History entry '{entry_id}' not found")
    return {"status": "deleted", "id": entry_id}


@router.delete("")
def clear_all():
    """Clear all history entries."""
    deleted = clear_history()
    return {"status": "cleared", "deleted": deleted}
