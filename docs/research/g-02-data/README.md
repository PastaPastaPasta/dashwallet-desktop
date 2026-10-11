# G-02 raw data (Tauri)

The data behind the Tauri column of [ADR 0003](../../adr/0003-ui-stack.md). Self-test reports are the
app's `--selftest` output; the gate scripts are in `ci/gate/` and `apps/desktop/scripts/gate/`.

| Folder | Where | Head | How |
|---|---|---|---|
| `linux/` | agentbox, load 120–122 (`linux-gate.uptime.txt`) | 9637840 binary (the `before-run` mark), gate scripts at 00a664a | `linux-gate.sh` on the 40k fixture; `u1.json` from `u1_compare.py` against `dwcli-list.txt` and `dwcli-history-200.txt`. `linux-packages.json`, `deb-info.txt`, `engine-open.txt` and `renderer-variants.txt` are from earlier heads of this branch (packaging, `dwcli` wallet open, the renderer check). |
| `ci-ubuntu/` | `ubuntu-latest` | 00a664a | `run.txt`; `RUNS=8 DROP_CACHES=4` |
| `ci-macos/` | `macos-latest` | 00a664a | `run.txt`; overlay scroll bars (`macos-scrollbars.txt`) |
| `ci-windows/` | `windows-latest` | 45e0038 (only `uia_walk.ps1` changed since 00a664a) | `run.txt`; `windows-gate.json`, `uia/` |
| `u3c/` | all | as above | `compare_boxes.py` outputs and the Playwright reports they use (`playwright-*`). `macos-1000x588-classic-scrollbars.json` is the 5c0f11c macOS run, before the gate turned on overlay scroll bars. |
| `screenshots/` | per folder above | as above | window captures of each screen, light and dark |

Sizes: PSS in KiB (`pssKb`), times in ms. Every agentbox number has its `uptime` next to it.
