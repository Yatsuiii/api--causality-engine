from __future__ import annotations
from pydantic import BaseModel, Field
from typing import Optional
from datetime import datetime


# ── Auth models ──────────────────────────────────────────────────────

class BasicAuth(BaseModel):
    username: str
    password: str


class ApiKeyAuth(BaseModel):
    header: str
    value: str


class OAuth2Config(BaseModel):
    token_url: str
    client_id: str
    client_secret: str
    scope: Optional[str] = None
    grant_type: Optional[str] = None


class Auth(BaseModel):
    bearer: Optional[str] = None
    basic: Optional[BasicAuth] = None
    api_key: Optional[ApiKeyAuth] = None
    oauth2: Optional[OAuth2Config] = None


# ── Step models ──────────────────────────────────────────────────────

class Transition(BaseModel):
    from_state: str = Field(alias="from")
    to_state: str = Field(alias="to")

    model_config = {"populate_by_name": True}


class RetryConfig(BaseModel):
    attempts: int
    delay_ms: int


class ValueCheck(BaseModel):
    eq: Optional[object] = None
    ne: Optional[object] = None
    contains: Optional[str] = None
    exists: Optional[bool] = None
    lt: Optional[float] = None
    gt: Optional[float] = None
    in_list: Optional[list] = None


class Assertion(BaseModel):
    status: Optional[int | dict] = None
    body: Optional[dict[str, ValueCheck | dict]] = None
    header: Optional[dict[str, ValueCheck | dict]] = None
    response_time_ms: Optional[ValueCheck | dict] = None


class Hook(BaseModel):
    set: Optional[dict[str, str]] = None
    log: Optional[str] = None
    delay_ms: Optional[int] = None
    skip_if: Optional[str] = None


class Step(BaseModel):
    name: str
    method: str
    url: str
    transition: Transition
    headers: Optional[dict[str, str]] = None
    body: Optional[object] = None
    extract: Optional[dict[str, str]] = None
    retry: Optional[RetryConfig] = None
    assertions: Optional[list[Assertion]] = Field(None, alias="assert")
    timeout_ms: Optional[int] = None
    pre_request: Optional[list[Hook]] = None
    post_request: Optional[list[Hook]] = None

    model_config = {"populate_by_name": True}


# ── Scenario ─────────────────────────────────────────────────────────

class Scenario(BaseModel):
    name: str
    initial_state: str
    steps: list[Step]
    concurrency: Optional[int] = None
    auth: Optional[Auth] = None
    variables: Optional[dict[str, str]] = None
    proxy: Optional[str] = None
    insecure: Optional[bool] = None
    default_timeout_ms: Optional[int] = None


# ── Execution results ────────────────────────────────────────────────

class AssertionResult(BaseModel):
    description: str = ""
    passed: bool = True
    expected: Optional[str] = None
    actual: Optional[str] = None


class StepLog(BaseModel):
    step_name: str = ""
    state_before: str = ""
    state_after: str = ""
    method: str = ""
    url: str = ""
    status: int = 0
    duration_ms: int = 0
    assertions: list[AssertionResult] = []
    request_body: Optional[str] = None
    response_body: Optional[str] = None


class ExecutionLog(BaseModel):
    steps: list[StepLog] = []
    total_duration_ms: int = 0
    total_steps: int = 0
    passed: int = 0
    failed: int = 0


# ── Environment ──────────────────────────────────────────────────────

class Environment(BaseModel):
    name: str
    variables: dict[str, str] = {}


# ── History ──────────────────────────────────────────────────────────

class HistoryEntry(BaseModel):
    id: str
    scenario_name: str
    scenario_file: str
    environment: Optional[str] = None
    started_at: str
    duration_ms: int = 0
    total_steps: int = 0
    passed: int = 0
    failed: int = 0
    log: ExecutionLog = ExecutionLog()
