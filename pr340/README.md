# Issue 340 UI artifacts

Fixture backend (`?scenario=funded`), Chromium, 1100×900. Fixture data only.

- `card-<theme>-before-9c186a474.png`: Settings ▸ Backups ▸ Automatic backups at the merge-base, after a `backupFailed` notice: the card as today, no failure row.
- `card-<theme>-after-a4cfcb025.png`: the same at the PR head a4cfcb025, with a failure recorded for the wallet "Main": the "The latest automatic backup failed" row.
- `toast-<theme>-before-9c186a474.png`: the whole window just after the notice at the merge-base: no toast.
- `toast-<theme>-after-a4cfcb025.png`: the same at the PR head a4cfcb025: the toast "An automatic backup failed. Open Settings ▸ Backups to see what to do."
