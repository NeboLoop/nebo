---
name: gws-drive
description: "Google Drive: Create, upload, and share files."
metadata:
  version: 0.22.5
  openclaw:
    category: "productivity"
    requires:
      bins:
        - gws
    cliHelp: "gws drive --help"
---

# drive

> **PREREQUISITE:** Read `../gws-shared/SKILL.md` for auth, global flags, and security rules.

Create, list, and manage Google Drive files. New Drive files are **private to the creator** until you grant access.

## Sharing (required before sending a link)

When the user will open, send, or mention a newly created/uploaded file to other people, ALWAYS grant those people access **before** sharing `webViewLink` / the Docs/Sheets URL. Prefer named people — do **not** default to `type=anyone`.

- Named person / email the user mentioned (edit):
  ```bash
  gws drive permissions create --params '{"fileId":"FILE_ID"}' --json '{"type":"user","role":"writer","emailAddress":"person@example.com"}'
  ```
- View-only for a named person:
  ```bash
  gws drive permissions create --params '{"fileId":"FILE_ID"}' --json '{"type":"user","role":"reader","emailAddress":"person@example.com"}'
  ```
- Repeat once per recipient. If you are about to email the file, grant every `--to` / `--cc` address first (see `gws-gmail-send`).

## Common create / upload

```bash
gws drive +upload ./report.pdf --name 'Q1 Report'
gws drive files create --json '{"name":"Notes","mimeType":"application/vnd.google-apps.document"}'
```

## Tips

- After create/upload, read `id` and `webViewLink` from the response; grant access, then share the link.
- Use `role=writer` when collaborators need to edit; `reader` when they only need to view.

> [!CAUTION]
> Permission changes are **write** commands — confirm with the user before granting access.

## See Also

- [gws-shared](../gws-shared/SKILL.md) — Global flags and auth
- [gws-drive-upload](../gws-drive-upload/SKILL.md) — Upload helper
- [gws-gmail-send](../gws-gmail-send/SKILL.md) — Email after granting recipients access
