"""Environment routes — CRUD for named variable sets."""

from typing import Optional

from fastapi import APIRouter, HTTPException
from pydantic import BaseModel, Field

from models import Environment
from services.storage import (
    delete_environment,
    get_environment,
    list_environments,
    save_environment,
)

router = APIRouter()


class CreateEnvironmentBody(BaseModel):
    name: str
    variables: dict[str, str] = Field(default_factory=dict)


class UpdateEnvironmentBody(BaseModel):
    variables: dict[str, str]


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
def create_env(body: CreateEnvironmentBody):
    """Create a new environment."""
    if get_environment(body.name) is not None:
        raise HTTPException(409, f"Environment '{body.name}' already exists")
    env = Environment(name=body.name, variables=body.variables)
    save_environment(env)
    return env.model_dump()


@router.put("/{name}")
def update_env(name: str, body: UpdateEnvironmentBody):
    """Update an existing environment's variables."""
    existing = get_environment(name)
    if existing is None:
        raise HTTPException(404, f"Environment '{name}' not found")
    updated = Environment(name=existing.name, variables=body.variables)
    save_environment(updated)
    return updated.model_dump()


@router.delete("/{name}")
def delete_env(name: str):
    """Delete an environment."""
    if not delete_environment(name):
        raise HTTPException(404, f"Environment '{name}' not found")
    return {"status": "deleted", "name": name}
