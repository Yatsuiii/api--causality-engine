// This file was initially hand-written from the Rust type definitions in
// ui/tauri/src/{storage,commands/}.  It will be overwritten by
// tauri-specta's automatic export the first time `cargo tauri dev` runs.
// Do not edit manually once the generated version exists.

export type AssertionResult = {
  description: string;
  passed: boolean;
  expected: string;
  actual: string;
};

export type StepLog = {
  step_name: string;
  state_before: string;
  state_after: string;
  method: string;
  url: string;
  status: number;
  duration_ms: number;
  assertions: AssertionResult[];
  request_body: string | null;
  response_body: string | null;
};

export type ExecutionLog = {
  steps: StepLog[];
  total_duration_ms: number;
  total_steps: number;
  passed: number;
  failed: number;
  iterations: number;
  terminal_state: string | null;
};

export type Environment = {
  name: string;
  variables: Record<string, string>;
};

export type HistoryEntry = {
  id: string;
  scenario_name: string;
  scenario_file: string;
  environment: string | null;
  started_at: string;
  duration_ms: number;
  total_steps: number;
  passed: number;
  failed: number;
  log: ExecutionLog;
};

export type ScenarioSummary = {
  file: string;
  name: string;
  steps: number;
  initial_state: string;
  concurrency: number | null;
  error: string | null;
};

export type ScenarioFile = {
  file: string;
  scenario: unknown;
};

export type RawScenario = {
  file: string;
  content: string;
};

export type DuplicatedScenario = {
  file: string;
  original: string;
  status: string;
};

export type ValidationResult = {
  valid: boolean;
  errors: string[];
};
