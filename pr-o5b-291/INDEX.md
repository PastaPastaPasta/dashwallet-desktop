# Outcome live region (issue 291): before and after

- Before: exact base `50c8e4f05dfdb4c63281aad4d43249332f8fd9e2` (the PR's merge base)
- After: head `90fd3aec85daa9597281202c39573ac09a02819d` (fix/291-outcome-live-region)
- Fixture backend, scenario `dashpay-registered`, Chromium (Playwright image), 1280×820.
- Nothing visible changes. The empty pairs are pixel-identical. The result pairs differ only by antialiasing (at most 4/255 per channel), with no layout change.

| State | Theme | Before (base 50c8e4f05) | After (head 90fd3aec8) |
|---|---|---|---|
| Add contact, Mallory found, before Send (result empty) | light | before/light-add-contact-empty-base-50c8e4f05.png | after/light-add-contact-empty-head-90fd3aec8.png |
| Add contact, Mallory found, before Send (result empty) | dark | before/dark-add-contact-empty-base-50c8e4f05.png | after/dark-add-contact-empty-head-90fd3aec8.png |
| Add contact, after Send: "Contact request sent to Mallory." | light | before/light-add-contact-sent-base-50c8e4f05.png | after/light-add-contact-sent-head-90fd3aec8.png |
| Add contact, after Send: "Contact request sent to Mallory." | dark | before/dark-add-contact-sent-base-50c8e4f05.png | after/dark-add-contact-sent-head-90fd3aec8.png |
| Bell open, before Accept (result empty) | light | before/light-bell-empty-base-50c8e4f05.png | after/light-bell-empty-head-90fd3aec8.png |
| Bell open, before Accept (result empty) | dark | before/dark-bell-empty-base-50c8e4f05.png | after/dark-bell-empty-head-90fd3aec8.png |
| Bell, after Accept heidi: "You and heidi are now contacts." | light | before/light-bell-accepted-base-50c8e4f05.png | after/light-bell-accepted-head-90fd3aec8.png |
| Bell, after Accept heidi: "You and heidi are now contacts." | dark | before/dark-bell-accepted-base-50c8e4f05.png | after/dark-bell-accepted-head-90fd3aec8.png |
