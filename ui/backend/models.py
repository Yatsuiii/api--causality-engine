from __future__ import annotations
from typing import Literal

from pydantic import BaseModel, Field


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
    scope: str | None = None
    grant_type: str | None = None


class Auth(BaseModel):
    bearer: str | None = None
    basic: BasicAuth | None = None
    api_key: ApiKeyAuth | None = None
    oauth2: OAuth2Config | None = None


# ── Step models ──────────────────────────────────────────────────────

class Transition(BaseModel):
    from_state: str = Field(alias="from")
    to_state: str = Field(alias="to")

    model_config = {"populate_by_name": True}


class RetryConfig(BaseModel):
    attempts: int
    delay_ms: int


class ValueCheck(BaseModel):
    eq: object | None = None
    ne: object | None = None
    contains: str | None = None
    exists: bool | None = None
    lt: float | None = None
    gt: float | None = None
    in_list: list | None = Field(default=None, alias="in")

    model_config = {"populate_by_name": True}


class Assertion(BaseModel):
    status: int | dict | None = None
    body: dict[str, ValueCheck | dict] | None = None
    header: dict[str, ValueCheck | dict] | None = None
    response_time_ms: ValueCheck | dict | None = None


class Hook(BaseModel):
    set: dict[str, str] | None = None
    log: str | None = None
    delay_ms: int | None = None
    skip_if: str | None = None


class MultipartFieldDef(BaseModel):
    name: str
    value: str | None = None
    file: str | None = None
    filename: str | None = None
    mime: str | None = None


class TransitionCondition(BaseModel):
    status: int | ValueCheck | dict | None = None
    body: dict[str, ValueCheck | dict] | None = None
    assertions: Literal["passed", "failed"] | None = None


class TransitionEdge(BaseModel):
    to: str
    when: TransitionCondition | None = None
    default: bool | None = None


class Step(BaseModel):
    name: str
    method: str
    url: str
    transition: Transition | None = None
    transitions: list[TransitionEdge] | None = None
    state: str | None = None
    headers: dict[str, str] | None = None
    body: object | None = None
    multipart: list[MultipartFieldDef] | None = None
    extract: dict[str, str] | None = None
    retry: RetryConfig | None = None
    assertions: list[Assertion] | None = Field(None, alias="assert")
    timeout_ms: int | None = None
    pre_request: list[Hook] | None = None
    post_request: list[Hook] | None = None

    model_config = {"populate_by_name": True}


# ── Scenario ─────────────────────────────────────────────────────────

class Scenario(BaseModel):
    name: str
    initial_state: str
    steps: list[Step]
    concurrency: int | None = None
    auth: Auth | None = None
    variables: dict[str, str] | None = None
    proxy: str | None = None
    insecure: bool | None = None
    default_timeout_ms: int | None = None
    max_iterations: int | None = None
    terminal_states: list[str] | None = None


# ── Execution results ────────────────────────────────────────────────

class AssertionResult(BaseModel):
    description: str = ""
    passed: bool = True
    expected: str | None = None
    actual: str | None = None


class StepLog(BaseModel):
    step_name: str = ""
    state_before: str = ""
    state_after: str = ""
    method: str = ""
    url: str = ""
    status: int = 0
    duration_ms: int = 0
    assertions: list[AssertionResult] = Field(default_factory=list)
    request_body: str | None = None
    response_body: str | None = None


class ExecutionLog(BaseModel):
    steps: list[StepLog] = Field(default_factory=list)
    total_duration_ms: int = 0
    total_steps: int = 0
    passed: int = 0
    failed: int = 0


# ── Environment ──────────────────────────────────────────────────────

class Environment(BaseModel):
    name: str
    variables: dict[str, str] = Field(default_factory=dict)


# ── History ──────────────────────────────────────────────────────────

class HistoryEntry(BaseModel):
    id: str
    scenario_name: str
    scenario_file: str
    environment: str | None = None
    started_at: str
    duration_ms: int = 0
    total_steps: int = 0
    passed: int = 0
    failed: int = 0
    log: ExecutionLog = Field(default_factory=ExecutionLog)
