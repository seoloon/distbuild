import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { Message } from "../bindings/Message";

/**
 * Job event channel names emitted by the Master's `submit_job` command
 * (see `apps/desktop/src-tauri/src/ws_client.rs::run_job_stream`).
 */
export type JobEventChannel =
  "job://log" | "job://progress" | "job://artifact" | "job://finished";

function onJobEvent(
  channel: JobEventChannel,
  handler: (message: Message) => void,
): Promise<UnlistenFn> {
  return listen<Message>(channel, (event) => handler(event.payload));
}

export function onJobLog(
  handler: (message: Message) => void,
): Promise<UnlistenFn> {
  return onJobEvent("job://log", handler);
}

export function onJobProgress(
  handler: (message: Message) => void,
): Promise<UnlistenFn> {
  return onJobEvent("job://progress", handler);
}

export function onJobArtifact(
  handler: (message: Message) => void,
): Promise<UnlistenFn> {
  return onJobEvent("job://artifact", handler);
}

export function onJobFinished(
  handler: (message: Message) => void,
): Promise<UnlistenFn> {
  return onJobEvent("job://finished", handler);
}
