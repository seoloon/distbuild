import type { Profile } from "./JobForm";

interface BuildButtonProps {
  selectedWorkerId: () => string | null;
  repo: () => string;
  branch: () => string;
  profile: () => Profile;
  disabled?: () => boolean;
  onSubmit: (
    workerId: string,
    repo: string,
    branch: string,
    profile: Profile,
  ) => void;
}

function isPlausibleRepoUrl(value: string): boolean {
  const trimmed = value.trim();
  if (trimmed.length === 0) return false;
  // Accepts URL-shaped strings (https://..., git://...) and the scp-like
  // git remote form (user@host:path) — a light sanity check, not full
  // validation; the real check happens when the worker actually clones it.
  return /^([a-zA-Z][a-zA-Z0-9+.-]*:\/\/|[\w.-]+@[\w.-]+:).+/.test(trimmed);
}

function BuildButton(props: BuildButtonProps) {
  const canBuild = () =>
    props.selectedWorkerId() !== null &&
    isPlausibleRepoUrl(props.repo()) &&
    !(props.disabled?.() ?? false);

  return (
    <button
      class="rounded bg-blue-600 px-4 py-2 text-sm font-medium text-white disabled:cursor-not-allowed disabled:opacity-40"
      disabled={!canBuild()}
      onClick={() => {
        const workerId = props.selectedWorkerId();
        if (!workerId) return;
        props.onSubmit(workerId, props.repo(), props.branch(), props.profile());
      }}
    >
      Build
    </button>
  );
}

export default BuildButton;
