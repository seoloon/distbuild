import { invoke } from "@tauri-apps/api/core";
import type { DiscoveredWorkerDto } from "../bindings/DiscoveredWorkerDto";
import type { PairingChallenge } from "../bindings/PairingChallenge";
import type { PairedWorkerDto } from "../bindings/PairedWorkerDto";

export function discoverWorkers(): Promise<DiscoveredWorkerDto[]> {
  return invoke("discover_workers");
}

export function pairWorker(
  host: string,
  port: number,
): Promise<PairingChallenge> {
  return invoke("pair_worker", { host, port });
}

export function confirmPair(
  host: string,
  port: number,
  code: string,
): Promise<PairedWorkerDto> {
  return invoke("confirm_pair", { host, port, code });
}

export function submitJob(
  workerId: string,
  repo: string,
  branch: string,
  profile: string,
): Promise<string> {
  return invoke("submit_job", { workerId, repo, branch, profile });
}

export function cancelJob(jobId: string): Promise<void> {
  return invoke("cancel_job", { jobId });
}

export function openArtifactsFolder(jobId: string): Promise<void> {
  return invoke("open_artifacts_folder", { jobId });
}
