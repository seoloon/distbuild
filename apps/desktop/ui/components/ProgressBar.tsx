import { createEffect, createSignal, onCleanup, onMount, Show } from "solid-js";
import { onJobFinished, onJobProgress } from "../lib/events";

interface ProgressBarProps {
  activeJobId: () => string | null;
}

const PHASE_LABELS: Record<string, string> = {
  cloning: "Cloning",
  deps: "Installing dependencies",
  building: "Building",
  packaging: "Packaging",
};

function ProgressBar(props: ProgressBarProps) {
  const [phase, setPhase] = createSignal<string | null>(null);
  const [startedAt, setStartedAt] = createSignal<number | null>(null);
  const [elapsedMs, setElapsedMs] = createSignal(0);
  const [finished, setFinished] = createSignal(false);

  // Reset whenever a new job becomes active.
  createEffect(() => {
    props.activeJobId();
    setPhase(null);
    setStartedAt(null);
    setElapsedMs(0);
    setFinished(false);
  });

  onMount(() => {
    const unlistenProgress = onJobProgress((message) => {
      if (message.type !== "JobProgress") return;
      if (message.job_id !== props.activeJobId()) return;
      setPhase(message.phase);
      if (startedAt() === null) setStartedAt(Date.now());
    });
    const unlistenFinished = onJobFinished((message) => {
      if (message.type !== "JobFinished") return;
      if (message.job_id !== props.activeJobId()) return;
      setFinished(true);
    });
    onCleanup(() => {
      unlistenProgress.then((fn) => fn());
      unlistenFinished.then((fn) => fn());
    });

    const interval = setInterval(() => {
      const started = startedAt();
      if (started !== null && !finished()) {
        setElapsedMs(Date.now() - started);
      }
    }, 250);
    onCleanup(() => clearInterval(interval));
  });

  return (
    <Show when={props.activeJobId()}>
      <div class="flex items-center gap-3 border-t border-neutral-800 px-4 py-2 text-xs text-neutral-400">
        <span>
          {phase() ? (PHASE_LABELS[phase()!] ?? phase()) : "Starting…"}
        </span>
        <span>{(elapsedMs() / 1000).toFixed(1)}s</span>
        <Show when={finished()}>
          <span class="text-green-400">Finished</span>
        </Show>
      </div>
    </Show>
  );
}

export default ProgressBar;
