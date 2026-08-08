import { createSignal } from "solid-js";
import MasterTab from "./tabs/MasterTab";
import WorkerTab from "./tabs/WorkerTab";
import "./App.css";

type Tab = "master" | "worker";

function App() {
  const [tab, setTab] = createSignal<Tab>("master");

  return (
    <main class="flex h-screen w-screen flex-col bg-neutral-950 text-neutral-100">
      <nav class="flex border-b border-neutral-800">
        <button
          class={`px-4 py-2 text-sm ${tab() === "master" ? "border-b-2 border-neutral-100" : "text-neutral-500"}`}
          onClick={() => setTab("master")}
        >
          Master
        </button>
        <button
          class={`px-4 py-2 text-sm ${tab() === "worker" ? "border-b-2 border-neutral-100" : "text-neutral-500"}`}
          onClick={() => setTab("worker")}
        >
          Worker
        </button>
      </nav>
      <section class="flex-1 overflow-auto">
        {tab() === "master" ? <MasterTab /> : <WorkerTab />}
      </section>
    </main>
  );
}

export default App;
