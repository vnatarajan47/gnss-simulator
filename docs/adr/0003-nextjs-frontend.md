# ADR-0003: Next.js for the frontend

Date: 2026-08-09
Status: Accepted

## Context
The frontend needs to support an extensible set of views as phases are added:
a map + sky plot (phase 1-2), mask configuration UI (phase 3), IQ generation
controls (phase 4), and dynamic trajectory input (phase 5). The frontend
framework choice needed to balance ease of iteration now against not
requiring a rewrite as views are added later.

## Decision
Use Next.js (App Router, TypeScript) as the frontend framework.

## Alternatives considered
- Vite + React: lighter weight, less framework overhead, arguably better
  suited to phase 1-2's needs alone (an interactive map/plot tool doesn't
  lean on server-side rendering).
- Next.js: chosen for its file-based routing conventions, which will pay off
  once additional views (masking, IQ config, trajectory input) are added in
  later phases, and for being the more broadly directionally standard choice
  going forward.

## Consequences
- Phase 1 will use Next.js mostly as a React shell (SSR is not heavily
  utilized for an interactive client-side tool), which is acknowledged as
  slightly heavier than strictly necessary at this stage.
- File-based routing will simplify adding new top-level views/pages as later
  phases introduce new capabilities.
- WASM modules must be loaded in client components (`"use client"`) via
  dynamic `import()`, since WASM instantiation is async and incompatible
  with server-side rendering.