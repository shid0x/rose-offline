# AGENTS.md

This file applies to the `rose-offline` repository.

Use this guide for server, shared gameplay, data-loading, protocol, and tooling work inside this repo.

## Repo Role

`rose-offline` is the Rust server workspace for the project.

It contains:
- the server executable
- shared gameplay/data/protocol crates
- iROSE-specific data and network implementations
- file readers for original ROSE formats
- utility tools for conversion, VFS inspection, and mip generation

This repo is the source of truth for most non-rendering game logic.

## Workspace Layout

- `rose-offline-server/`
  Main server executable and runtime integration layer.
  Contains:
  - startup / CLI
  - protocol server wiring
  - ECS game world
  - systems, events, resources, storage, bots

- `rose-file-readers/`
  Parsers and VFS support for ROSE file formats.
  Examples include `STB`, `STL`, `QSD`, `CHR`, `ZON`, `ZSC`, `ZMO`, `ZMS`, `AIP`, and multiple VFS variants.
  Start here when the task is about binary/text asset decoding or virtual filesystem behavior.

- `rose-data/`
  Shared game-data structures and databases loaded from files.
  This is the normalized data layer used by gameplay/network code.

- `rose-data-irose/`
  iROSE-specific data loading and interpretation.
  Use this when the change depends on iROSE asset conventions or version-specific decoding.

- `rose-game-common/`
  Shared gameplay-facing types: components, messages, and common gameplay data helpers.

- `rose-game-irose/`
  iROSE-specific gameplay formulas and rules built on top of shared game/data crates.

- `rose-network-common/`
  Shared packet/connection abstractions.

- `rose-network-irose/`
  iROSE packet definitions and codecs.
  Start here for packet field layout, encoding/decoding, or version-specific protocol behavior.

- `rose-offline-tools/`
  Utility binaries:
  - `rose-conv`
  - `rose-vfs-dump`
  - `rose-dds-mipgen`

## Dependency Direction

Keep changes in the lowest correct layer.

Typical flow:
- `rose-file-readers`
- `rose-data` / `rose-data-irose`
- `rose-game-common` / `rose-game-irose`
- `rose-network-common` / `rose-network-irose`
- `rose-offline-server`

Guidance:
- If a bug is about parsing a raw file format, do not patch around it in server code first.
- If a rule is gameplay-specific, prefer `rose-game-*` over packet or storage code.
- If a problem is packet shape or codec behavior, prefer `rose-network-*`.
- Use `rose-offline-server` for orchestration, ECS wiring, runtime flow, persistence, and integration.

## Where To Put Changes

- File-format and VFS bugs:
  - `rose-file-readers`

- Database loading, string tables, quest/product/item/NPC data:
  - `rose-data`
  - `rose-data-irose` when it is iROSE-specific

- Shared gameplay data structures and components:
  - `rose-game-common`

- iROSE formulas, drop tables, stat calculations, version-specific gameplay logic:
  - `rose-game-irose`

- Generic packet transport or connection behavior:
  - `rose-network-common`

- iROSE packet definitions and codec behavior:
  - `rose-network-irose`

- Server systems, events, storage, commands, AI, zone startup, login/world/game server flow:
  - `rose-offline-server`

- Developer utilities and one-off data tooling:
  - `rose-offline-tools`

## Server Notes

Inside `rose-offline-server`:
- `src/main.rs` handles CLI, logging, data source setup, and server boot.
- `src/game/` contains most gameplay runtime logic.
- `src/game/systems/` is the main place for gameplay behavior changes.
- `src/game/events/` and `src/game/messages/` define event/message flow.
- `src/game/resources/` holds shared runtime state.
- `src/game/storage/` contains persistence-related logic.
- `src/irose/protocol/` is the bridge between network packets and game actions.

Prefer extending existing systems/resources/events instead of creating parallel logic paths.

## Data Notes

- The server can load from `data.idx`, extracted data directories, or both.
- Many bugs that look like server logic bugs are actually data-decoding issues upstream.
- `QSD`, `STB`, `STL`, AI, and zone-related formats often affect quests, NPC behavior, strings, shops, spawns, and triggers.
- When investigating content issues, inspect both the reader layer and the consuming database/gameplay layer.

## Validation

Run commands from the `rose-offline/` repo root.

Prefer targeted validation:
- `cargo check -p rose-offline-server`
- `cargo check -p rose-file-readers`
- `cargo test -p rose-file-readers`
- `cargo test -p rose-offline-server`
- `cargo test -p rose-game-common`
- `cargo test -p rose-game-irose`
- `cargo test -p rose-data-irose`

Use narrower filters when possible for faster iteration, especially in heavily tested server systems.

## Working Rules

- Do not edit `target/`.
- Avoid large refactors across multiple crates unless the task clearly requires it.
- Preserve existing crate boundaries; do not move version-specific behavior into common crates without a strong reason.
- When touching packet, data, and gameplay layers together, verify the direction of the fix before editing all three.
- Keep parser changes conservative; many formats are reverse-engineered and small changes can ripple widely.

## Common Pitfalls

- Fixing file-format issues in higher layers instead of the reader/data layer.
- Mixing generic logic with iROSE-specific behavior.
- Changing packet structs without checking the corresponding encode/decode path and server integration.
- Making server-system changes without checking for tests in the same file or module.
- Assuming a runtime bug is in ECS logic when the loaded asset/database values are wrong.

## When Unsure

- Start in `rose-file-readers` for raw format questions.
- Start in `rose-data-irose` for iROSE data interpretation.
- Start in `rose-network-irose` for packet compatibility issues.
- Start in `rose-offline-server/src/game/systems` for gameplay behavior seen on the server.

