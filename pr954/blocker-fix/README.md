# PR 954: contested shared registration and confirming spend back

Fixture captures (Chromium, 1280x1000, the fixture's fixed clock), light and dark.

| Column | Revision |
|---|---|
| base | bfe8160719b5d83a220750550abbb2b111c04707 (merge base with main) |
| prev | 4769956a1c0948306e127af7327a20e8d841dc9d (the PR head the review found the problem at) |
| head | ef17f6f953fc483b10a96511bf56fa661f0eda78 (PR head) |

Each revision ran in its own checkout with two fixture scenarios added for the capture only, each holding the session that revision's engine reports for the same chain state:

- contested: coordinator, registration sent, another spend of a reserved coin in a block that isn't final (2 confirmations). base: stage conflicted (it settled on any block), so the session leaves In progress. prev: stage sent, next wait. head: stage sent, next nothing, confirming other_spend 2.
- spendback: participant, out and abandoned, its spend back in a block that isn't final (1 confirmation). base: stage conflicted, leaves In progress. prev: stage funding_signed, next nothing, spend back with no state. head: as prev plus confirming abandon_tx 1.

## sha256

```
3e627657c5a89659bdea6a0bf1d5dfe743da2382371b7e54182863d7cf2b2763  base/contested-list-dark.png
26d11caa5e040ea786312bc6c6962e02daf16434e7916ca287f5b2c56be94fac  base/contested-list-light.png
3e627657c5a89659bdea6a0bf1d5dfe743da2382371b7e54182863d7cf2b2763  base/spendback-list-dark.png
26d11caa5e040ea786312bc6c6962e02daf16434e7916ca287f5b2c56be94fac  base/spendback-list-light.png
3dbd62518f73d710bd891d795bd96dbd2285cf6475177ecf89ccc28ec34266c7  head/contested-dark.png
ae9416fdcf244b9efe2073f6051f66c944d0a67b648d46effc61eecad4a9e4e1  head/contested-light.png
ae8f191d5a592b86045a6132c6248a794693e7c93cbd91b6dea9361012c0cd45  head/contested-list-dark.png
4a55250adb9330f0d7e8b4d7f597c3d82e2875d03d4122c877778d2b734348d4  head/contested-list-light.png
bcba0864e15972f8f40d34fc197820d8af53f0f61e93b50dacd037887ee4f6cc  head/spendback-dark.png
3aa01bb2a5dc5d0bb3e2550435e2cb09ca79222e6310ef324378e1c3f8dc1496  head/spendback-light.png
b8351ae7c4abc0fc5fac9c684770cc2264396ce3e1c38bf56c7945da294567a9  prev/contested-dark.png
ab422ec7a2f5b30df82636a7dfd3edb326146daa2809e2eef77dda8ad10dc14c  prev/contested-light.png
3e23d99354ec591f3cde760e173e1f151a817dc1c80ac669463b890d3c67a97f  prev/spendback-dark.png
6b749bc2f3866558ec0e97b104c75cef389b77a8787f618ac1e54d783655affb  prev/spendback-light.png
```
