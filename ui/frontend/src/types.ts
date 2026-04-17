// Types derived from Rust structs — sourced from the generated bindings.
// Re-exported here so existing imports from "../types" continue to work.
export type {
  AssertionResult,
  DuplicatedScenario,
  Environment,
  ExecutionLog,
  HistoryEntry,
  RawScenario,
  ScenarioFile,
  StepLog,
} from "./bindings";

// ScenarioSummary is the Rust name; export it under the legacy TS name too.
export type { ScenarioSummary, ScenarioSummary as ScenarioListItem } from "./bindings";

/* ── Validation Results ───────────────────────────────────────────── */

// Kept in its original shape so App.tsx can display stdout/stderr output.
// useApi.ts maps the Tauri command's `errors: string[]` into these fields.
export interface ValidationResult {
  valid: boolean;
  stdout: string;
  stderr: string;
  exit_code: number;
}

/* ── Auth ─────────────────────────────────────────────────────────────── */

export interface BasicAuth {
  username: string;
  password: string;
}

export interface ApiKeyAuth {
  header: string;
  value: string;
}

export interface OAuth2Config {
  token_url: string;
  client_id: string;
  client_secret: string;
  scope?: string;
  grant_type?: string;
}

export interface Auth {
  bearer?: string;
  basic?: BasicAuth;
  api_key?: ApiKeyAuth;
  oauth2?: OAuth2Config;
}

/* ── Step ─────────────────────────────────────────────────────────────── */

export interface Edge {
  from: string;
  to: string;
  when?: TransitionCondition;
  default?: boolean;
}

export interface TransitionCondition {
  status?: number | ValueCheck | Record<string, unknown>;
  body?: Record<string, ValueCheck | Record<string, unknown>>;
  assertions?: "passed" | "failed";
}

export interface RetryConfig {
  attempts: number;
  delay_ms: number;
}

export interface MultipartFieldDef {
  name: string;
  value?: string;
  file?: string;
  filename?: string;
  mime?: string;
}

export interface ValueCheck {
  eq?: unknown;
  ne?: unknown;
  contains?: string;
  exists?: boolean;
  lt?: number;
  gt?: number;
  in?: unknown[];
}

export interface Assertion {
  status?: number | Record<string, unknown>;
  body?: Record<string, ValueCheck | Record<string, unknown>>;
  header?: Record<string, ValueCheck | Record<string, unknown>>;
  response_time_ms?: ValueCheck | Record<string, unknown>;
}

export interface Hook {
  set?: Record<string, string>;
  log?: string;
  delay_ms?: number;
  skip_if?: string;
}

export interface Step {
  name: string;
  state: string;
  method: HttpMethod;
  url: string;
  headers?: Record<string, string>;
  body?: unknown;
  multipart?: MultipartFieldDef[];
  extract?: Record<string, string>;
  retry?: RetryConfig;
  assert?: Assertion[];
  timeout_ms?: number;
  pre_request?: Hook[];
  post_request?: Hook[];
}

/* ── Scenario ────────────────────────────────────────────────────────── */

export interface Scenario {
  name: string;
  initial_state: string;
  steps: Step[];
  edges?: Edge[];
  concurrency?: number;
  auth?: Auth;
  variables?: Record<string, string>;
  proxy?: string;
  insecure?: boolean;
  default_timeout_ms?: number;
  max_iterations?: number;
  terminal_states?: string[];
}

export function isScenario(value: unknown): value is Scenario {
  if (typeof value !== "object" || value === null) return false;
  const obj = value as Record<string, unknown>;
  return (
    typeof obj.name === "string" &&
    typeof obj.initial_state === "string" &&
    Array.isArray(obj.steps)
  );
}

export type HttpMethod =
  | "GET"
  | "POST"
  | "PUT"
  | "PATCH"
  | "DELETE"
  | "HEAD"
  | "OPTIONS";

export const HTTP_METHODS: HttpMethod[] = [
  "GET",
  "POST",
  "PUT",
  "PATCH",
  "DELETE",
  "HEAD",
  "OPTIONS",
];
