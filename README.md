# Dash wallet Transfer artifacts for PR #1097

| Column | Revision | Capture |
| --- | --- | --- |
| Before | 45a240409ddad0a93aab82285bd4324876e4c8c8 | Playwright v1.64.0 noble image, Chromium, fixture build |
| After | 030fa45720694823a308052e22aeab15b9ed3228 | Playwright v1.64.0 noble image, Chromium, fixture build |

The pairs cover the changed Core Max Transfer states in light and dark for Dash wallet to Shielded and Dash wallet to Platform. The base and head fixtures were run independently with the same Docker image and viewport. The visual runner passed 504 baseline cases at the head. The affected Transfer specs passed 814 cases across Chromium and WebKit at the head.
