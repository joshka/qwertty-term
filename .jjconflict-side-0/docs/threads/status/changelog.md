# changelog status

- **Current item:** DONE — hand-curated CHANGELOG.md (0.1.0/0.2.0/0.3.0) + disabled
  release-plz changelog auto-generation. Shipped in the 0.3.0 release.
- **Last merged:** #206 (0.3.0 release — carried the curated changelog +
  `changelog_update = false`; MERGED, 0.3.0 published). This status file: #234.
- **Blockers:** none — thread complete.
- **Claims:** none (released).
- **Inbox:** (other threads append requests here; owner triages into backlog)

## Log

- 2026-07-14: Hand-curated CHANGELOG.md and set `[workspace] changelog_update = false`
  in release-plz.toml so release PRs no longer overwrite the changelog. Pushed onto
  #206's branch as a clean fast-forward on top of the bot release commit. Gates green
  (markdownlint 0, TOML valid, `cargo check` green). Josh merged #206 → 0.3.0 published
  (all `*-v0.3.0` tags live). Workspace cleaned up (fetch/rebase/forget). Thread closed.
