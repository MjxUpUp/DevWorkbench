import { invoke } from '@tauri-apps/api/core';

/**
 * Mission Mode (D4) frontend API — typed wrappers over the `mission_*` Tauri
 * commands. The lifecycle:
 *   init (Phase 1) → agent writes prd.json → loadPrd (validate) → apply
 *   (Phase 2) → status (poll). State is persisted by the backend in
 *   `~/.dev-workbench/missions/<id>/` and round-trips these camelCase shapes.
 *
 * Rust command params are snake_case (`mission_id`); Tauri converts to the
 * camelCase keys the JS side passes here (`missionId`).
 */
export type MissionPhase =
  | 'plan'
  | 'executing'
  | 'completed'
  | 'max_iterations_reached';

export interface MissionState {
  currentPhase: MissionPhase;
  iteration: number;
  maxIterations: number;
}

export interface MissionLoadResult {
  valid: boolean;
  problems: string[];
  prd: Record<string, unknown> | null;
  corrupted: boolean;
}

export interface MissionStatusView {
  state: MissionState;
  passed: number;
  total: number;
  corrupted: boolean;
}

export const missionApi = {
  /** Phase 1 start — stage the mission dir + plan-phase state.json. */
  init: (missionId: string) =>
    invoke<MissionState>('mission_init', { missionId }),
  /** Read + validate the agent's prd.json (valid iff problems is empty). */
  loadPrd: (missionId: string) =>
    invoke<MissionLoadResult>('mission_load_prd', { missionId }),
  /** User confirmed the PRD → flip to Phase 2 (executing). Errors if invalid. */
  apply: (missionId: string) =>
    invoke<MissionState>('mission_apply', { missionId }),
  /** Live status: current phase/iteration + pass count over total stories. */
  status: (missionId: string) =>
    invoke<MissionStatusView>('mission_status', { missionId }),
};
