# DistBuild

*Build anywhere. Receive locally.*

A LAN-first remote build & artifact distribution tool. A **Master** machine
sends a build job (Git URL + branch + profile) to a **Worker** machine; the
Worker clones, detects the build system, compiles for its own native
platform, streams logs in real time, and returns the artifacts as a zip.

See `docs/superpowers/specs/2026-08-08-distbuild-design.md` for the full
design.

## Development

Requires: Rust (stable), Node.js, pnpm, [`just`](https://github.com/casey/just).

    just dev     # run the desktop app in dev mode
    just build   # build workspace + desktop app for release
    just fmt     # format Rust + frontend
    just test    # run the Rust test suite
