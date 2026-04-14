/**
 * Typed wrappers for Tauri commands.
 * Each function is a single invoke() call — no HTTP, no proxy.
 */

import { invoke } from "@tauri-apps/api/core";

import type { Scenario, ValidationResult } from "../types";
import type {
  ScenarioSummary,
  RawScenario,
  DuplicatedScenario,
  Environment,
  HistoryEntry,
} from "../bindings";

/* ── Health ───────────────────────────────────────────────────────── */

// Tauri commands are always "up" — there is no backend process to check.
// This stub exists so call sites in App.tsx don't need to change.
export async function checkHealth(): Promise<{ status: string }> {
  return { status: "ok" };
}

/* ── Scenarios ───────────────────────────────────────────────────────── */

export async function fetchScenarios(): Promise<ScenarioSummary[]> {
  return invoke("list_scenarios");
}

export async function fetchScenario(
  name: string
): Promise<{ file: string; scenario: Scenario }> {
  return invoke("get_scenario", { name });
}

export async function fetchScenarioRaw(name: string): Promise<RawScenario> {
  return invoke("get_scenario_raw", { name });
}

export async function saveScenarioRaw(
  name: string,
  content: string
): Promise<{ file: string; status: string }> {
  const file = await invoke<string>("update_scenario_raw", { name, content });
  return { file, status: "updated" };
}

export async function createScenario(
  scenario: Partial<Scenario> & { name: string }
): Promise<{ file: string; status: string }> {
  const file = await invoke<string>("create_scenario", {
    name: scenario.name,
    scenario,
  });
  return { file, status: "created" };
}

export async function updateScenario(
  name: string,
  scenario: Scenario
): Promise<{ file: string; status: string }> {
  const file = await invoke<string>("update_scenario", { name, scenario });
  return { file, status: "updated" };
}

export async function deleteScenario(
  name: string
): Promise<{ file: string; status: string }> {
  const file = await invoke<string>("delete_scenario", { name });
  return { file, status: "deleted" };
}

export async function duplicateScenario(
  name: string
): Promise<DuplicatedScenario> {
  return invoke("duplicate_scenario", { name });
}

/* ── Runner ──────────────────────────────────────────────────────────── */

export async function runScenario(
  scenarioFile: string,
  environment?: string,
  variables?: Record<string, string>
): Promise<HistoryEntry> {
  return invoke("run_scenario", {
    scenario_file: scenarioFile,
    environment: environment ?? null,
    variables: variables ?? null,
  });
}

export async function validateScenario(
  scenarioFile: string
): Promise<ValidationResult> {
  const result = await invoke<{ valid: boolean; errors: string[] }>(
    "validate_scenario",
    { scenario_file: scenarioFile }
  );
  return {
    valid: result.valid,
    stdout: result.errors.join("\n"),
    stderr: "",
    exit_code: result.valid ? 0 : 1,
  };
}

/* ── Environments ────────────────────────────────────────────────────── */

export async function fetchEnvironments(): Promise<Environment[]> {
  return invoke("list_environments");
}

export async function fetchEnvironment(name: string): Promise<Environment> {
  return invoke("get_environment", { name });
}

export async function createEnvironment(
  name: string,
  variables: Record<string, string> = {}
): Promise<Environment> {
  return invoke("create_environment", { name, variables });
}

export async function updateEnvironment(
  name: string,
  variables: Record<string, string>
): Promise<Environment> {
  return invoke("update_environment", { name, variables });
}

export async function deleteEnvironment(
  name: string
): Promise<{ status: string; name: string }> {
  await invoke("delete_environment", { name });
  return { status: "deleted", name };
}

/* ── History ──────────────────────────────────────────────────────────── */

export async function fetchHistory(limit = 50): Promise<HistoryEntry[]> {
  return invoke("list_history", { limit });
}

export async function fetchHistoryEntry(id: string): Promise<HistoryEntry> {
  return invoke("get_history_entry", { id });
}

export async function deleteHistoryEntry(
  id: string
): Promise<{ status: string; id: string }> {
  await invoke("delete_history_entry", { id });
  return { status: "deleted", id };
}

export async function clearHistory(): Promise<{
  status: string;
  deleted: number;
}> {
  const deleted = await invoke<number>("clear_history");
  return { status: "cleared", deleted };
}
