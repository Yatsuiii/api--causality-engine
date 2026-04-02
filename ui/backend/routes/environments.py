"""Environment routes — CRUD for named variable sets."""

from fastapi import APIRouter, HTTPException

from models import Environment
from services.storage import (
    list_environments,
    get_environment,
    save_environment,
    delete_environment,
)

router = APIRouter()


@router.get("")
def list_envs():
    """List all environments."""
    return [e.model_dump() for e in list_environments()]


@router.get("/{name}")
def get_env(name: str):
    """Get a single environment."""
    env = get_environment(name)
    if env is None:
        raise HTTPException(404, f"Environment '{name}' not found")
    return env.model_dump()


@router.post("")
def create_env(body: dict):
    """Create a new environment."""
    name = body.get("name")
    if not name:
        raise HTTPException(400, "Environment name is required")
    if get_environment(name) is not None:
        raise HTTPException(409, f"Environment '{name}' already exists")
    env = Environment(name=name, variables=body.get("variables", {}))
    save_environment(env)
    return env.model_dump()


@router.put("/{name}")
def update_env(name: str, body: dict):
    """Update an existing environment."""
    existing = get_environment(name)
    if existing is None:
        raise HTTPException(404, f"Environment '{name}' not found")
    existing.variables = body.get("variables", existing.variables)
    save_environment(existing)
    return existing.model_dump()


@router.delete("/{name}")
def delete_env(name: str):
    """Delete an environment."""
    if not delete_environment(name):
        raise HTTPException(404, f"Environment '{name}' not found")
    return {"status": "deleted", "name": name}
