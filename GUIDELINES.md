# Documentation Guidelines

<a id="top"></a>

Documents per project, in HTML — the development docs split by boundary:

| # | Document | Boundary | Content | Elsewhere |
|--:|----------|----------|---------|-----------|
| 1 | [README](#readme) | — | WHAT & WHY · user interactions | — |
| 2 | [USER](#user) | upper — end users / dependent software | protocols · conventions · APIs · workflows · UI & prototypes (graphical programs) | internal mechanics → [CORE](#core) · underlying sources → [PROVIDER](#provider) |
| 3 | [CORE](#core) | none — internal only; anything with no counterpart outside the project | internal data structures & APIs · call stacks · SBOM | usage-facing → [USER](#user) · provider boundary → [PROVIDER](#provider) |
| 4 | [PROVIDER](#provider) | lower — underlying data sources / dependencies (system services, IPC, hardware, daemons, network endpoints, …); omit when absent | protocols · conventions · APIs · workflows | whole-system view → [USER](#user) · internal handling → [CORE](#core) |
| 5 | [BUILD](#build) | written after development | how to use & build on the project | — |

**Proximity principle** — place a concern at the nearest boundary it directly faces; an indirect relationship does not move it to a higher document.

**End goal** — a set of design constraints immediately useful to any developer implementing the code.

**AVOID at all costs** — comparisons to historical design · rationale for a specific design choice (broad general principles excepted).

## Global constraints

[↑](#top)

| # | Constraint | Rule |
|--:|-----------|------|
| 1 | Single source of truth | a definition appears once, at the most appropriate spot; everywhere else → hyperlink |
| 2 | No amendment history | never record changes against previous versions — it only adds confusion |
| 3 | `reference/` | optional; stores official external documentation as the authoritative source of truth |
| 4 | HTML escaping | escape `&` `<` `>` as entities wherever they are content, not markup — `<pre>/<code>` especially: unescaped brackets are parsed as tags and silently deleted (`Result<T, E>`, ASCII arrows `<--`) |
| 5 | Notes | supplements under diagrams/tables/file trees/call stacks → `<ul class="note"><li>…</li></ul>` (token `--note` in style.css), never flowing prose |
| 6 | Section layout | development docs only — opening: qualifying prose (when structure is impossible) · middle: structured content only (code/diagrams/tables) · end: `ul.note` bullets only |
| 7 | Comment budget | an in-code comment with >3 information points → `ul.note` after its code block; the comment keeps only what the signature/call needs |
| 8 | One block, one flow | a code block is a single continuous content; disjoint pieces → separate blocks/subsections |
| 9 | CONVENTIONS chapter | USER / CORE / PROVIDER each keep one, right after the first chapter (may stay empty); stated once there, referenced from the affected sections |

## Readme

[↑](#top)

1. **WHAT & WHY** — IDEAL: the problem to solve · GOAL: what success looks like · non-goals (optional)
2. **User interactions** — only entry points and user-observable outcomes; do not explain any mechanisms behind them. Those belong to USER/CORE.
3. **Project map** — optional; root project of a multi-project repo only · ASCII tree of the project family
4. **Credits** — always present, always last; list the credited projects (and stack/license where useful), not what was referenced

## User

[↑](#top)

**Rules**
- finer requirements not expressible by a straightforward mechanism → complement at the end of the section; last resort — prefer mechanism over specific constraint
- USER states user-visible behavior only; internal mechanism names (cache, probe, queue construction) and engineering constants belong to CORE.

**Chapters**
1. **TERMS** — domain keywords, never synonymized, full definition; when used, no further explanation
2. **CONVENTIONS** — shared across interfaces (Global constraints #9)
3. **DIAGRAMS** — mechanisms are the preferred documentation form
   - general architecture — how the application interacts with others (Component Diagram)
   - page / URL navigation — Component Diagram with pages as nodes and navigation edges; do not maintain a route list in CONVENTIONS
   - workflows — business language; terms/API names only for well-known standards; Sequence Diagram / Activity Diagram with Swimlanes, form per [Diagrams](#diagrams-plantuml)
4. **INTERFACES**
   - exposed API specifications — code, or pseudocode for multi-language support
   - message types incl. errors — pseudocode definition block + table of meaning & suggested action
   - data models — objects of exposed APIs (bare code/pseudocode, no comments) · field table (external specs referenced) · transformations (exact data object names)
5. **UI** — optional; graphical programs only; all UI prototypes/mocks live here
   - prototype/mock — code, pictures or videos
   - palette — color tokens and their semantic roles
   - visual presentation — the per-component visual contract (geometry, states, styling); concrete requirements must be stated in prose/code; a prototype illustrates but does not replace the written/encoded contract, and exact numeric parity with the prototype is not required

## Core

[↑](#top)

**Rules**
- not exhaustive — key execution paths only: startup, key events, shutdown
- symbols already defined in USER/PROVIDER → one-line declaration with trailing `//见 USER §xx.xx` / `//见 PROVIDER §xx.xx` — project language, no body, no redefinition; everything else syntactically valid in the project language
- engineering constants (bus names, periods, capacities, …) — in the owning source file's signature subsection, declare the constant name and purpose; a concrete value may appear only as a recommended value in a comment (e.g. `// 推荐值 15_000`), never as an assignment in the declaration; do not use a separate constants table; tuned against the real system, code stays the operational source of truth; a literal may appear inline in a call stack where that flow depends on it; exception: UI visual literals (dimensions, radii, font sizes, …) pin the visual contract and may be stated in USER's UI chapters — CSS/code stays the operational source of truth
- no implementation reference → draft meaningful names/signatures marked `待实现，可以根据实际情况调整` (call-stack lines use the drafted names, no marker at the call site); a reference exists (main branch, breadpan, std / GTK APIs) → use its real names

**Chapters**
1. **File tree** — final code structure; each entry states what the module serves and links to its signatures subsection (no subsection → plain entry); lists source/build/config/assets only — documentation artifacts and their styling are out of scope
2. **CONVENTIONS** — shared across interfaces (Global constraints #9)
3. **SBOM** — dependency libraries in one table: name · what it serves in this project; version & license stay in the package manifest
4. **Internal types & method signatures** — one subsection per source file, in file-tree order, titled by file name, linking back to the file tree · methods: signature + at most a one-line purpose note — must cover every method involved in a core flow; non-core helper methods may be included at the author's discretion — internals (call chains, branching, fallbacks, ordering) belong to the call-stack section · data structures & messages: type definitions, the comments are the explanation — no field tables
5. **Call stack tree** — actual execution of key program logic
   - lines carry calls or judgments only — every line points at a concrete call (`module::fn(args)`, DBus method, event send, …); a judgment/branch condition is the sole exception
   - one line carries at most one call or one judgment; a judgment may point to its single resulting call; independent calls are separate sibling/child lines; equivalent entrypoints may share one line separated by `/`
   - event listeners are represented by the binding call statement; calls inside the listener are child calls, like any other call
   - a node's children are calls actually made by that node; a path shared by several callers is a separate root/subsection, never nested under one caller
   - call-tree structure follows the source: separately registered listeners stay separate; a semantic grouping such as `player.on(command)` is a note/table abstraction, not a substitute for actual registrations
   - at most one trailing comment per call — a comment is not a tree node (no `├──`/`└──` of its own), but the ancestor spine `│` continues through its line
   - motivations, policies, trade-offs beyond one comment line → prose after the block

## Provider

[↑](#top)

**Rules**
- contractual = the consumed subset only; the full authoritative definition is the official documentation (mirror in `reference/` when applicable) — linked, never copied
- workflow diagrams name the provider's methods/messages crossing the boundary; the project's internal call structure never appears in them

**Chapters**
1. **TERMS** — provider-specific keywords, never synonymized, full definition; when used, no further explanation
2. **CONVENTIONS** — shared across the provider boundary (Global constraints #9)
3. **DIAGRAMS** — boundary workflows; Sequence Diagram / Activity Diagram with Swimlanes, form per [Diagrams](#diagrams-plantuml)
4. **INTERFACES**
   - consumed API specifications — code, or pseudocode for multi-language support
   - message types sent/received on this boundary, incl. errors — pseudocode definition block + table of meaning & suggested action
   - data models — objects crossing this boundary (bare code/pseudocode, no comments) · field table (external spec as authority) · transformations between provider ↔ project data objects (exact names)

## Diagrams: PlantUML

[↑](#top)

Applies to USER & PROVIDER. Every diagram is drawn with PlantUML — Component Diagram, Sequence Diagram and Activity Diagram with Swimlanes alike; Mermaid and ASCII graphs are banned outright, actual pictures for everything else.
Sources (`*.puml`) and exports (`*.svg`) live in the owning project's own `diagram/` — the folder follows its project, never a repo-wide root; the SVG is embedded via `<img>` with a relative path (`diagram/…`).

1. **Form by complexity** — general architecture: Component Diagram · at most one branch decision (<2 nesting levels): Sequence Diagram · more branching: Activity Diagram with Swimlanes, one swimlane per participant
2. **Vertical layout** — branch-heavy flows as guard-clause cascades: checks run sequentially, each branch ends `stop` — never nested `if/else`, which PlantUML spreads side-by-side
3. **Labels** — in the documentation language; short messages, details into notes
4. **Source is authoritative** — edit `*.puml`, re-export (`plantuml -tsvg <project>/diagram/*.puml`), commit both together; never hand-edit the exported image, never let source and image diverge
5. **Embedding** — wrap in `<div class="diagram">`, natural size, horizontal scroll when wider than the page (same mode as `<pre>`); never scale down
6. **Participants** — draw the project as one participant; internal processes or components (main/renderer, views, stores, …) never appear. Only messages crossing the project boundary are drawn; internal processing is omitted or shown once as a self-loop on the project labeled `内部处理`, never expanded into internal call structure

## BUILD

[↑](#top)

**Rules**
- produced once development is complete — not written during the design phase, but kept as part of the project
- guides other developers in using and building on the project
