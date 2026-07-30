# ADR 0001: Language and Application Foundation

- **Status:** Accepted
- **Date:** 2026-07-29

## Decision

Agency will be built primarily in **Rust**.

The application will use a native, GPU-rendered Rust UI rather than a webview-based desktop shell. The first UI candidate is **Iced with its `wgpu` renderer**, subject to a short cross-platform prototype before it becomes a permanent dependency.

The embedded terminal will use **Ghostty's `libghostty-vt` engine** through safe Rust bindings. Agency will own the PTY, process lifecycle, input routing, and terminal renderer.

## Why Rust

Agency sits at the intersection of several systems-heavy concerns:

- long-running and interactive child processes
- PTYs and platform-specific process behavior
- Git repositories and worktrees
- filesystem observation
- local persistence and credentials
- concurrent agent sessions
- high-volume terminal output
- large, interactive diffs
- native packaging across Windows, macOS, and Linux

Rust is a strong common language for all of these. It allows Agency's domain model, provider adapters, process supervision, repository integration, and UI to share types and run in one application without a separate native sidecar or serialization boundary.

Rust also provides the most direct practical path to Ghostty integration without making Zig the language of the entire product. `libghostty-vt` exposes a C API, and safe Rust bindings already exist.

## Ghostty integration

“Based on Ghostty” specifically means:

- use `libghostty-vt` for escape-sequence parsing and terminal state
- consume its render-state API
- preserve Ghostty-compatible behavior where the library provides it
- contribute fixes upstream where practical

It does **not** mean embedding the existing Ghostty desktop application. Ghostty's application shells and renderers are platform-specific, and `libghostty-vt` does not provide a ready-made cross-platform UI widget.

Agency must supply:

- Unix PTY and Windows ConPTY integration
- process spawning, shutdown, and recovery
- a dedicated terminal-emulation thread per active terminal, communicating over channels
- keyboard, mouse, IME, clipboard, and link handling
- font discovery, shaping, fallback, and glyph caching
- terminal rendering through the application's GPU renderer
- accessibility semantics

The Ghostty Rust API is pre-1.0 and its terminal types are neither `Send` nor `Sync`. The integration must be isolated behind Agency-owned interfaces so upstream API changes do not spread through the application.

## UI foundation

### Initial choice: Iced + wgpu

Iced is the leading bootstrap candidate because it:

- officially targets Windows, macOS, and Linux
- is written in Rust
- supports custom widgets
- provides a `wgpu` renderer across Vulkan, Metal, and DirectX 12
- allows the terminal and diff surfaces to share a native GPU rendering stack
- avoids placing a webview between Agency and terminal rendering

Iced is still described by its maintainers as experimental. Therefore this ADR accepts the **native Rust GPU UI direction**, while treating the exact framework choice as provisional until the bootstrap spike passes.

### Required framework spike

Before building product UI, create one window that runs on all three target operating systems and demonstrates:

1. a resizable split layout
2. a large virtualized file list
3. a syntax-highlighted diff with selectable text
4. a `libghostty-vt` terminal backed by a real PTY or ConPTY
5. terminal text selection, clipboard, Unicode, IME, mouse reporting, and resize/reflow
6. keyboard focus and shortcut routing between terminal and application
7. basic screen-reader semantics

Keep Iced only if the same architecture passes on Windows, macOS, and Linux without platform-specific product behavior.

## Why not the main alternatives

### TypeScript with Electron

Electron would accelerate conventional interface work, but the most demanding Agency surface—the terminal—would either use a browser terminal implementation or require a difficult native-surface integration. It also creates a permanent TypeScript/native boundary around Git, PTYs, agents, and repository state.

This is a reasonable fallback if native Rust UI development blocks product delivery, but it is not the preferred foundation.

### Tauri with a web frontend

Tauri is attractive for distribution size and Rust backend code. It still places the main interface inside platform webviews, whose behavior differs across operating systems. Integrating Ghostty's render state into that composition model adds complexity precisely where Agency needs predictable rendering, input, and focus.

Tauri remains a possible fallback shell if Agency later chooses a web-rendered terminal.

### Zig

Zig would make direct Ghostty integration natural, but Agency's broader needs—GitHub APIs, structured persistence, asynchronous orchestration, mature application libraries, and a large contributor-facing codebase—favor Rust's ecosystem. Ghostty's C API removes the need to adopt Zig application-wide.

### GPUI

GPUI is compelling for IDE-class interfaces and is proven by Zed. It remains pre-1.0, has limited standalone documentation, and its own README does not yet present Windows as a supported development target. Agency should not make equal Windows support depend on reverse-engineering another application's internal framework.

It should be reconsidered if GPUI publishes stable, independently documented support for all three desktop platforms.

## Proposed workspace boundaries

The initial Rust workspace should separate durable product logic from replaceable infrastructure:

```text
crates/
  agency-domain/       # provider-neutral product model and workflows
  agency-store/        # local persistent state
  agency-git/          # repositories, branches, diffs, and worktrees
  agency-github/       # GitHub forge and issue adapter
  agency-agents/       # agent provider contracts and supervision
  agency-codex/        # Codex adapter
  agency-claude/       # Claude Code adapter
  agency-terminal/     # PTY/ConPTY and libghostty-vt integration
  agency-ui/           # framework-independent presentation state
  agency-desktop/      # windowing, rendering, and packaging
```

The boundaries are more important than the exact number of crates. In particular, no UI component should call GitHub, Git, or agent CLIs directly.

## Consequences

### Benefits

- one primary language across product logic and native infrastructure
- strong process and concurrency safety
- direct access to platform APIs when required
- a credible high-performance terminal and diff experience
- minimal impedance between Ghostty state and GPU rendering
- provider-neutral domain types shared across the application

### Costs and risks

- native Rust UI ecosystems are less mature than web UI ecosystems
- Agency must build substantial terminal rendering and input integration
- `libghostty-vt` and its Rust bindings can introduce breaking changes
- accessibility and platform polish require deliberate work
- all three operating systems must be continuously tested from the first prototype

## Reversal strategy

Keep the domain, Git, GitHub, agent, persistence, and terminal-session layers independent of the desktop framework. If Iced fails the spike, replace `agency-desktop` without rewriting Agency's core.

If direct Ghostty rendering proves too expensive, preserve the terminal interface and temporarily substitute another terminal renderer. Do not let the terminal implementation dictate the rest of the product architecture.
