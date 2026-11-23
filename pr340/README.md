# Issue 340 UI artifacts

Fixture backend (`?scenario=funded`), Chromium, 1100×900. Fixture data only.

- `card-<theme>-before-b942e212d.png`: Settings ▸ Backups ▸ Automatic backups at the merge-base, after a `backupFailed` notice: the card as today, no failure row.
- `card-<theme>-after-c21c48fa4.png`: the same at the PR head, with a failure recorded for the wallet "Main": the "The latest automatic backup failed" row.
- `toast-<theme>-before-b942e212d.png`: the whole window just after the notice at the merge-base: no toast.
- `toast-<theme>-after-c21c48fa4.png`: the same at the PR head: the toast "An automatic backup failed. Open Settings ▸ Backups to see what to do."
