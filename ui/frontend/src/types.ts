/* ── Auth ─────────────────────────────────────────────────────────── */

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

/* ── Step ─────────────────────────────────────────────────────────── */

export interface Transition {
  from: string;
  to: string;
}

export interface TransitionCondition {
  status?: number | ValueCheck | Record<string, unknown>;
  body?: Record<string, ValueCheck | Record<string, unknown>>;
  assertions?: "passed" | "failed";
}

export interface TransitionEdge {
  to: string;
  when?: TransitionCondition;
  default?: boolean;
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
  method: HttpMethod;
  url: string;
  transition?: Transition;
  transitions?: TransitionEdge[];
  state?: string;
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

/* ── Scenario ────────────────────────────────────────────────────── */

export interface Scenario {
  name: string;
  initial_state: string;
  steps: Step[];
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

/* ── Execution Results ───────────────────────────────────────────── */

export interface AssertionResult {
  description: string;
  passed: boolean;
  expected?: string;
  actual?: string;
}

export interface StepLog {
  step_name: string;
  state_before: string;
  state_after: string;
  method: string;
  url: string;
  status: number;
  duration_ms: number;
  assertions: AssertionResult[];
  request_body?: string;
  response_body?: string;
}

export interface ExecutionLog {
  steps: StepLog[];
  total_duration_ms: number;
  total_steps: number;
  passed: number;
  failed: number;
}

/* ── Environment ─────────────────────────────────────────────────── */

export interface Environment {
  name: string;
  variables: Record<string, string>;
}

/* ── History ──────────────────────────────────────────────────────── */

export interface HistoryEntry {
  id: string;
  scenario_name: string;
  scenario_file: string;
  environment?: string;
  started_at: string;
  duration_ms: number;
  total_steps: number;
  passed: number;
  failed: number;
  log: ExecutionLog;
}

/* ── Validation Results ───────────────────────────────────────────── */

export interface ValidationResult {
  valid: boolean;
  stdout: string;
  stderr: string;
  exit_code: number;
}

/* ── API list item ───────────────────────────────────────────────── */

export interface ScenarioListItem {
  file: string;
  name: string;
  steps: number;
  initial_state: string;
  concurrency?: number;
  error?: string;
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
