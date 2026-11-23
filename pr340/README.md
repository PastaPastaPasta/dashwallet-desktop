# Issue 340 UI artifacts

Fixture backend (`?scenario=funded`), Chromium, 1100×900. Fixture data only.

- `card-<theme>-before-fd8751bff.png`: Settings ▸ Backups ▸ Automatic backups at the merge-base fd8751bff, after a `backupFailed` notice: the card as today, no failure row.
- `card-<theme>-after-35f2d5719.png`: the same at the PR head 35f2d5719, with a failure recorded for the wallet "Main": the "The latest automatic backup failed" row.
- `toast-<theme>-before-fd8751bff.png`: the whole window just after the notice at the merge-base: no toast.
- `toast-<theme>-after-35f2d5719.png`: the same at the PR head 35f2d5719: the toast "An automatic backup failed. Open Settings ▸ Backups to see what to do."
