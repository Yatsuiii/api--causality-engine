/**
 * Typed fetch wrapper for the ACE backend API.
 * All calls go through the Vite proxy (/api → localhost:18710).
 */

import type {
  ScenarioListItem,
  Scenario,
  Environment,
  HistoryEntry,
} from "../types";

const BASE = "/api";

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(`${BASE}${path}`, {
    headers: { "Content-Type": "application/json", ...init?.headers },
    ...init,
  });
  if (!res.ok) {
    const body = await res.text();
    throw new Error(`${res.status}: ${body}`);
  }
  return res.json();
}

/* ── Health ───────────────────────────────────────────────────────── */

export async function checkHealth(): Promise<{ status: string }> {
  return request("/health");
}

/* ── Workspace ───────────────────────────────────────────────────── */

export async function getWorkspace(): Promise<{ workspace: string }> {
  return request("/workspace");
}

export async function setWorkspace(
  path: string
): Promise<{ workspace: string }> {
  return request("/workspace", {
    method: "POST",
    body: JSON.stringify({ path }),
  });
}

/* ── Scenarios ───────────────────────────────────────────────────── */

export async function fetchScenarios(): Promise<ScenarioListItem[]> {
  return request("/scenarios");
}

export async function fetchScenario(
  name: string
): Promise<{ file: string; scenario: Scenario }> {
  return request(`/scenarios/${encodeURIComponent(name)}`);
}

export async function fetchScenarioRaw(
  name: string
): Promise<{ file: string; content: string }> {
  return request(`/scenarios/${encodeURIComponent(name)}/raw`);
}

export async function saveScenarioRaw(
  name: string,
  content: string
): Promise<{ file: string; status: string }> {
  return request(`/scenarios/${encodeURIComponent(name)}/raw`, {
    method: "PUT",
    body: JSON.stringify({ content }),
  });
}

export async function createScenario(
  scenario: Partial<Scenario> & { name: string }
): Promise<{ file: string; status: string }> {
  return request("/scenarios", {
    method: "POST",
    body: JSON.stringify({ name: scenario.name, scenario }),
  });
}

export async function updateScenario(
  name: string,
  scenario: Scenario
): Promise<{ file: string; status: string }> {
  return request(`/scenarios/${encodeURIComponent(name)}`, {
    method: "PUT",
    body: JSON.stringify({ scenario }),
  });
}

export async function deleteScenario(
  name: string
): Promise<{ file: string; status: string }> {
  return request(`/scenarios/${encodeURIComponent(name)}`, {
    method: "DELETE",
  });
}

export async function duplicateScenario(
  name: string
): Promise<{ file: string; status: string; original: string }> {
  return request(`/scenarios/${encodeURIComponent(name)}/duplicate`, {
    method: "POST",
  });
}

/* ── Runner ──────────────────────────────────────────────────────── */

export async function runScenario(
  scenarioFile: string,
  environment?: string,
  variables?: Record<string, string>
): Promise<HistoryEntry> {
  return request("/runner/run", {
    method: "POST",
    body: JSON.stringify({
      scenario_file: scenarioFile,
      environment,
      variables,
    }),
  });
}

export async function validateScenario(
  scenarioFile: string
): Promise<{
  valid: boolean;
  stdout: string;
  stderr: string;
  exit_code: number;
}> {
  return request("/runner/validate", {
    method: "POST",
    body: JSON.stringify({ scenario_file: scenarioFile }),
  });
}

/* ── Environments ────────────────────────────────────────────────── */

export async function fetchEnvironments(): Promise<Environment[]> {
  return request("/environments");
}

export async function fetchEnvironment(name: string): Promise<Environment> {
  return request(`/environments/${encodeURIComponent(name)}`);
}

export async function createEnvironment(
  name: string,
  variables: Record<string, string> = {}
): Promise<Environment> {
  return request("/environments", {
    method: "POST",
    body: JSON.stringify({ name, variables }),
  });
}

export async function updateEnvironment(
  name: string,
  variables: Record<string, string>
): Promise<Environment> {
  return request(`/environments/${encodeURIComponent(name)}`, {
    method: "PUT",
    body: JSON.stringify({ variables }),
  });
}

export async function deleteEnvironment(
  name: string
): Promise<{ status: string; name: string }> {
  return request(`/environments/${encodeURIComponent(name)}`, {
    method: "DELETE",
  });
}

/* ── History ──────────────────────────────────────────────────────── */

export async function fetchHistory(limit = 50): Promise<HistoryEntry[]> {
  return request(`/history?limit=${limit}`);
}

export async function fetchHistoryEntry(id: string): Promise<HistoryEntry> {
  return request(`/history/${encodeURIComponent(id)}`);
}

export async function deleteHistoryEntry(
  id: string
): Promise<{ status: string; id: string }> {
  return request(`/history/${encodeURIComponent(id)}`, { method: "DELETE" });
}

export async function clearHistory(): Promise<{
  status: string;
  deleted: number;
}> {
  return request("/history", { method: "DELETE" });
}
