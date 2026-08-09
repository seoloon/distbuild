import { createSignal } from "solid-js";
import WorkerList, { type PairedWorker } from "../components/WorkerList";
import JobForm, { type Profile } from "../components/JobForm";
import BuildButton from "../components/BuildButton";
import ProgressBar from "../components/ProgressBar";
import LogViewer from "../components/LogViewer";
import { submitJob } from "../lib/commands";

function MasterTab() {
  const [pairedWorkers, setPairedWorkers] = createSignal<PairedWorker[]>([]);
  const [selectedWorkerId, setSelectedWorkerId] = createSignal<string | null>(null);

  const [repo, setRepo] = createSignal("");
  const [branch, setBranch] = createSignal("main");
  const [profile, setProfile] = createSignal<Profile>("debug");

  const [activeJobId, setActiveJobId] = createSignal<string | null>(null);
  const [submitError, setSubmitError] = createSignal<string | null>(null);

  async function handleSubmit(workerId: string, repoUrl: string, branchName: string, profileValue: Profile) {
    setSubmitError(null);
    try {
      const jobId = await submitJob(workerId, repoUrl, branchName, profileValue);
      setActiveJobId(jobId);
    } catch (e) {
      setSubmitError(String(e));
    }
  }

  return (
    <div class="grid h-full grid-cols-[280px_1fr] overflow-hidden">
      <div class="overflow-y-auto border-r border-neutral-800">
        <WorkerList
          selectedWorkerId={selectedWorkerId}
          onSelect={setSelectedWorkerId}
          pairedWorkers={pairedWorkers}
          onPaired={(worker) => setPairedWorkers((prev) => [...prev, worker])}
        />
      </div>
      <div class="flex flex-col overflow-hidden">
        <JobForm
          repo={repo}
          setRepo={setRepo}
          branch={branch}
          setBranch={setBranch}
          profile={profile}
          setProfile={setProfile}
        />
        <div class="flex items-center gap-3 px-4 pb-3">
          <BuildButton
            selectedWorkerId={selectedWorkerId}
            repo={repo}
            branch={branch}
            profile={profile}
            onSubmit={handleSubmit}
          />
          {submitError() && <span class="text-xs text-red-400">{submitError()}</span>}
        </div>
        <ProgressBar activeJobId={activeJobId} />
        <div class="min-h-0 flex-1">
          <LogViewer />
        </div>
      </div>
    </div>
  );
}

export default MasterTab;
