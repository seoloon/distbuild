export type Profile = "debug" | "release";

interface JobFormProps {
  repo: () => string;
  setRepo: (v: string) => void;
  branch: () => string;
  setBranch: (v: string) => void;
  profile: () => Profile;
  setProfile: (v: Profile) => void;
}

function JobForm(props: JobFormProps) {
  return (
    <div class="space-y-3 p-4">
      <div>
        <label class="mb-1 block text-xs text-neutral-400">
          Repository URL
        </label>
        <input
          class="w-full rounded border border-neutral-700 bg-neutral-900 px-2 py-1 text-sm text-neutral-100"
          placeholder="https://github.com/user/repo.git"
          value={props.repo()}
          onInput={(e) => props.setRepo(e.currentTarget.value)}
        />
      </div>
      <div>
        <label class="mb-1 block text-xs text-neutral-400">Branch</label>
        <input
          class="w-full rounded border border-neutral-700 bg-neutral-900 px-2 py-1 text-sm text-neutral-100"
          value={props.branch()}
          onInput={(e) => props.setBranch(e.currentTarget.value)}
        />
      </div>
      <div>
        <label class="mb-1 block text-xs text-neutral-400">Profile</label>
        <div class="flex gap-4 text-sm text-neutral-300">
          <label class="flex items-center gap-1">
            <input
              type="radio"
              name="profile"
              checked={props.profile() === "debug"}
              onChange={() => props.setProfile("debug")}
            />
            Debug
          </label>
          <label class="flex items-center gap-1">
            <input
              type="radio"
              name="profile"
              checked={props.profile() === "release"}
              onChange={() => props.setProfile("release")}
            />
            Release
          </label>
        </div>
      </div>
    </div>
  );
}

export default JobForm;
