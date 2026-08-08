# Prompt Claude Code — DistBuild

```
You are a senior Rust + TypeScript engineer with deep experience in Tauri v2, Axum, Tokio, WebSockets, mDNS/Zeroconf, and SolidJS. You will bootstrap and iteratively build a cross-platform desktop application called **DistBuild**.

═══════════════════════════════════════════════════════════
## MISSION
═══════════════════════════════════════════════════════════

Build a LAN-first remote build & artifact distribution tool.
- A **Master** machine sends a build job (Git URL + branch + profile) to a **Worker** machine.
- The Worker clones, detects the build system, compiles for ITS OWN native platform, streams logs in real-time, and returns the artifacts as a zip.
- Zero-config: mDNS discovery, 6-digit pairing, ultra-minimal UI (2 tabs).

Tagline: *Build anywhere. Receive locally.*

═══════════════════════════════════════════════════════════
## TECH STACK (STRICT)
═══════════════════════════════════════════════════════════

- Tauri v2 (single binary contains both Master + Worker roles, toggled by UI tab)
- Rust backend: Axum + Tokio + tokio-tungstenite (WebSocket)
- Discovery: `mdns-sd` crate (Zeroconf/Bonjour compatible)
- Git: `git2` crate (libgit2 bindings)
- Frontend: SolidJS + Vite + Tailwind CSS v4
- Serialization: `serde` + `serde_json`
- Logging: `tracing` + `tracing-subscriber`
- TLS: `rustls` with self-signed cert generated on first run (`rcgen`)
- Zip: `zip` crate

═══════════════════════════════════════════════════════════
## MONOREPO LAYOUT
═══════════════════════════════════════════════════════════

distbuild/
├── apps/
│   ├── desktop/                  # Tauri v2 app (src-tauri + SolidJS in `ui/`)
│   │   ├── src-tauri/
│   │   └── ui/
│   └── worker-core/              # Headless worker daemon (optional CLI)
├── crates/
│   ├── discovery/                # mDNS broadcast + browse
│   ├── protocol/                 # WebSocket message types (shared)
│   ├── executor/                 # Build system detection + spawn + log streaming
│   ├── artifacts/                # Glob collection + zip packaging
│   └── diagnostics/              # Toolchain detection (rustc, node, xcode, etc.)
├── docs/
├── Cargo.toml                    # workspace
└── README.md

═══════════════════════════════════════════════════════════
## PROTOCOL (crate: protocol)
═══════════════════════════════════════════════════════════

Define ALL message types with `#[derive(Serialize, Deserialize)]` and a `type` tag:

Client → Worker:
- `PairRequest { master_name, master_id }`
- `PairConfirm { code }`
- `JobRequest { job_id, repo, branch, profile, env?, distbuild_toml? }`
- `JobCancel { job_id }`

Worker → Master:
- `PairChallenge { code_shown_on_worker }`
- `PairAccepted { worker_id, worker_name, os, arch }`
- `JobStarted { job_id, timestamp }`
- `LogChunk { job_id, stream: "stdout"|"stderr", data, ts }`
- `JobProgress { job_id, phase: "cloning"|"deps"|"building"|"packaging", pct? }`
- `JobFinished { job_id, success, duration_ms, exit_code }`
- `ArtifactReady { job_id, filename, size_bytes, sha256 }`
- `Error { code, message }`

Use an enum `Message` with `#[serde(tag = "type")]`.

═══════════════════════════════════════════════════════════
## BUILD DETECTION (crate: executor)
═══════════════════════════════════════════════════════════

Priority order:
1. `distbuild.toml` → full override (name, command, env, artifact globs)
2. `src-tauri/tauri.conf.json` → `cargo tauri build`
3. `Cargo.toml` → `cargo build --release` (or `--profile <profile>`)
4. `bun.lockb` → `bun install && bun run build`
5. `pnpm-lock.yaml` → `pnpm install && pnpm build`
6. `package.json` → `npm ci && npm run build`
7. `Makefile` → `make build`

Spawn with `tokio::process::Command`, capture stdout+stderr line-by-line, forward as `LogChunk`.

═══════════════════════════════════════════════════════════
## ARTIFACTS (crate: artifacts)
═══════════════════════════════════════════════════════════

Default globs by build type:
- Tauri: `src-tauri/target/{profile}/bundle/**/*.{app,dmg,exe,msi,deb,AppImage,rpm}`
- Cargo: `target/{profile}/{binary_name}[.exe]`
- Node: `dist/**/*`, `build/**/*`, `out/**/*`

Zip → `<worker>/jobs/<job_id>/artifacts.distbuild.zip`, stream over WebSocket in binary frames with a small header `{ job_id, chunk_index, total_chunks }`.

═══════════════════════════════════════════════════════════
## UI (SolidJS + Tailwind)
═══════════════════════════════════════════════════════════

Two tabs ONLY: **Master** and **Worker**. Dark by default, native macOS/Windows chrome, single window ~900×650.

Master tab components:
- `<WorkerList />` — live list from mDNS, status dot (🟢 idle / 🟡 busy / 🔴 offline)
- `<JobForm />` — repo URL, branch (default `main`), profile (Debug/Release radio), destination folder picker
- `<BuildButton />` — disabled unless a worker is selected + repo URL valid
- `<LogViewer />` — virtualized, ANSI color support (use `ansi_up`), auto-scroll toggle
- `<ProgressBar />` — phase + elapsed time

Worker tab components:
- `<WorkerToggle />` — Start/Stop broadcasting
- `<SystemInfo />` — OS, arch, hostname
- `<ToolchainStatus />` — rustc, cargo, node, npm, bun, pnpm, xcode, make (green/red pills)
- `<WorkDir />` — path + "Open in Finder/Explorer"
- `<CacheManager />` — sizes + "Clear" per cache

Tauri commands (Rust ↔ JS) via `#[tauri::command]`.

═══════════════════════════════════════════════════════════
## SECURITY (v1 scope only)
═══════════════════════════════════════════════════════════

- 6-digit pairing code shown on Worker, entered on Master
- Store paired peers in `~/Documents/DistBuild/peers.json` (persistent trust)
- WSS with self-signed cert (rcgen), pinned per-peer after pairing
- Reject non-loopback / non-private IPs (RFC1918 + link-local only)
- Confirm job dialog on Worker before executing (auto-accept toggle)

═══════════════════════════════════════════════════════════
## DIRECTORY LAYOUT (runtime)
═══════════════════════════════════════════════════════════

~/Documents/DistBuild/
├── repositories/<repo-name>/
├── cache/{cargo,node,tauri}/
├── jobs/<job_id>/
├── artifacts/<job_id>/
├── peers.json
└── config.toml

═══════════════════════════════════════════════════════════
## DELIVERY PLAN — WORK IN THIS ORDER
═══════════════════════════════════════════════════════════

**Phase 0 — Scaffold**
1. `cargo new --lib` all crates + Cargo workspace root
2. `pnpm create tauri-app` in `apps/desktop` (SolidJS template, TS)
3. Wire Tailwind v4, base layout with 2 tabs
4. CI-ready `justfile` with `just dev`, `just build`, `just fmt`, `just test`

**Phase 1 — Protocol + Discovery**
5. Implement `protocol` crate with full Message enum + roundtrip tests
6. Implement `discovery` crate: broadcast `_distbuild._tcp.local.` with TXT records (name, os, arch, port, version), browse from Master
7. Integration test: two processes on localhost see each other in < 1s

**Phase 2 — Worker daemon**
8. Axum server with `/ws` upgrade handler
9. Pairing flow (challenge/confirm, persist to peers.json)
10. Executor crate: detection + spawn + line-buffered log streaming
11. Artifacts crate: glob collection + zip + chunked binary send

**Phase 3 — Master UI**
12. Tauri commands: `discover_workers`, `pair_worker`, `submit_job`, `cancel_job`, `open_artifacts_folder`
13. WebSocket client with auto-reconnect + resume on `JobStarted` reception
14. LogViewer with virtualization (window > 10k lines)

**Phase 4 — Polish**
15. Diagnostics crate + Worker tab toolchain pills
16. `distbuild.toml` parser + override logic
17. Error taxonomy + user-friendly messages
18. README with GIF demo + install instructions

═══════════════════════════════════════════════════════════
## NON-NEGOTIABLE RULES
═══════════════════════════════════════════════════════════

1. **Never invent cross-compilation.** A Worker builds ONLY for its own OS. Surface this clearly in UI.
2. **All async code uses Tokio.** No blocking IO in handlers — wrap with `spawn_blocking` where needed (git2).
3. **Log every job to disk** at `jobs/<job_id>/{stdout.log,stderr.log,manifest.json}` even if Master disconnects.
4. **Zero-config default.** If the user hasn't set anything, everything must Just Work on the same Wi-Fi.
5. **Type-safe protocol.** JS side uses generated TS types (use `ts-rs` or `specta`) from Rust definitions — no hand-written duplicates.
6. **Write tests** for: protocol serialization, build detection, artifact globbing, discovery roundtrip.
7. **Idiomatic Rust:** `thiserror` for errors, `Result<T, DistBuildError>` everywhere, no `.unwrap()` outside tests, no `panic!` in library code.
8. **Format & lint:** `cargo fmt`, `cargo clippy -- -D warnings`, `prettier`, `eslint` all clean before committing.
9. **Commit convention:** Conventional Commits (`feat:`, `fix:`, `chore:`, `refactor:`, `test:`, `docs:`).
10. **Ask before deviating** from the stack or architecture above.

═══════════════════════════════════════════════════════════
## START NOW
═══════════════════════════════════════════════════════════

Begin with **Phase 0 — Scaffold**. Show me:
1. The workspace `Cargo.toml`
2. The folder tree you'll create
3. The `justfile`
4. The initial `apps/desktop/src-tauri/Cargo.toml`

Then wait for my "go" before implementing Phase 1.
```

---

Ce prompt est calibré pour Claude Code : phases séquentielles, contraintes explicites, checkpoints de validation (le "wait for go" évite les dérives), et rappel des non-négociables techniques (Tokio partout, pas de cross-compil magique, types partagés Rust↔TS).