# Check of #862 on testnet at 45c07adb4d490865aababf510be55bdfbe244ee3

Release build 0.3.0-dev.1.g45c07ad, `--network testnet`, fresh data directory, fixture testnet wallet restored (Core 4.99997923, Shielded 4.9734972 tDASH).

| Path | State reached | Reason line | Confirm |
| --- | --- | --- | --- |
| Send from Shielded (Pay, shielded address) | entry, form, confirm with quote (fee 0.00114140) | none | enabled |
| Shielded to Dash wallet (Transfer) | form, confirm with quote (fee 0.00226480) | none | enabled |
| Shielded to Platform (Transfer) | form, confirm with quote (fee 0.00120222) | none | enabled |

No path was disabled, and the Transfer picker offered Shielded with no "Not available in this version" text. Confirm was not pressed.

Files: `{light,dark}-send-{entry,form,confirm}.png`, `{light,dark}-out-{form,confirm}.png`, `{light,dark}-toPlatform-{form,confirm}.png`.
