import { createResource, createSignal, For, Show } from "solid-js";
import { confirmPair, discoverWorkers, pairWorker } from "../lib/commands";
import type { DiscoveredWorkerDto } from "../bindings/DiscoveredWorkerDto";

export interface PairedWorker {
  workerId: string;
  workerName: string;
  host: string;
  port: number;
}

interface WorkerListProps {
  selectedWorkerId: () => string | null;
  onSelect: (workerId: string) => void;
  pairedWorkers: () => PairedWorker[];
  onPaired: (worker: PairedWorker) => void;
}

function WorkerList(props: WorkerListProps) {
  const [workers, { refetch }] = createResource(discoverWorkers);
  const [pairingTarget, setPairingTarget] =
    createSignal<DiscoveredWorkerDto | null>(null);
  // NOTE: the pairing code is shown here for lack of a Worker tab (Phase 4)
  // to display it out-of-band. The real security model calls for the
  // human to read the code off the *worker's own* screen and type it
  // here — displaying it in the Master UI is a stopgap so pairing is
  // testable before that UI exists, and should be reconsidered once it does.
  const [challengeCode, setChallengeCode] = createSignal<string | null>(null);
  const [codeInput, setCodeInput] = createSignal("");
  const [error, setError] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal(false);

  function pairedEntryFor(host: string | undefined, port: number) {
    if (!host) return undefined;
    return props
      .pairedWorkers()
      .find((w) => w.host === host && w.port === port);
  }

  async function startPairing(worker: DiscoveredWorkerDto) {
    setError(null);
    const address = worker.addresses[0];
    if (!address) {
      setError("worker has no reachable address");
      return;
    }
    setBusy(true);
    try {
      const challenge = await pairWorker(address, worker.port);
      setPairingTarget(worker);
      setChallengeCode(challenge.code);
      setCodeInput("");
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function confirm() {
    const worker = pairingTarget();
    if (!worker) return;
    const address = worker.addresses[0];
    if (!address) return;
    setBusy(true);
    try {
      const paired = await confirmPair(address, worker.port, codeInput());
      props.onPaired({
        workerId: paired.worker_id,
        workerName: paired.worker_name,
        host: address,
        port: worker.port,
      });
      props.onSelect(paired.worker_id);
      setPairingTarget(null);
      setChallengeCode(null);
      setCodeInput("");
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div class="p-4">
      <div class="mb-2 flex items-center justify-between">
        <h2 class="text-sm font-medium text-neutral-300">Workers</h2>
        <button
          class="text-xs text-neutral-400 hover:text-neutral-200"
          onClick={() => refetch()}
        >
          Refresh
        </button>
      </div>
      <Show when={error()}>
        <p class="mb-2 text-xs text-red-400">{error()}</p>
      </Show>
      <ul class="space-y-1">
        <For each={workers() ?? []}>
          {(worker) => {
            const entry = () =>
              pairedEntryFor(worker.addresses[0], worker.port);
            const isPaired = () => entry() !== undefined;
            return (
              <li class="flex items-center justify-between rounded border border-neutral-800 px-2 py-1 text-sm">
                <button
                  class="flex items-center gap-2 text-left disabled:opacity-50"
                  disabled={!isPaired()}
                  onClick={() => {
                    const paired = entry();
                    if (paired) props.onSelect(paired.workerId);
                  }}
                >
                  <span>{isPaired() ? "🟢" : "⚪"}</span>
                  <span
                    class={
                      props.selectedWorkerId() === entry()?.workerId
                        ? "text-white"
                        : "text-neutral-300"
                    }
                  >
                    {worker.name} ({worker.os}/{worker.arch})
                  </span>
                </button>
                <Show when={!isPaired()}>
                  <button
                    class="text-xs text-blue-400 hover:text-blue-300 disabled:opacity-50"
                    disabled={busy()}
                    onClick={() => startPairing(worker)}
                  >
                    Pair
                  </button>
                </Show>
              </li>
            );
          }}
        </For>
      </ul>
      <Show when={pairingTarget()}>
        {(worker) => (
          <div class="mt-3 rounded border border-neutral-700 p-3 text-sm">
            <p class="mb-2 text-neutral-300">
              Enter the 6-digit code shown on {worker().name}:
            </p>
            <p class="mb-2 text-xs text-neutral-500">Code: {challengeCode()}</p>
            <input
              class="mb-2 w-full rounded border border-neutral-700 bg-neutral-900 px-2 py-1 text-neutral-100"
              value={codeInput()}
              maxLength={6}
              onInput={(e) => setCodeInput(e.currentTarget.value)}
            />
            <div class="flex gap-2">
              <button
                class="rounded bg-blue-600 px-2 py-1 text-xs text-white disabled:opacity-50"
                disabled={busy() || codeInput().length !== 6}
                onClick={confirm}
              >
                Confirm
              </button>
              <button
                class="rounded border border-neutral-700 px-2 py-1 text-xs text-neutral-300"
                onClick={() => {
                  setPairingTarget(null);
                  setChallengeCode(null);
                }}
              >
                Cancel
              </button>
            </div>
          </div>
        )}
      </Show>
    </div>
  );
}

export default WorkerList;
