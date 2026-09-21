---
name: gws-drive-upload
description: "Google Drive: Upload a local file."
metadata:
  version: 0.22.5
  openclaw:
    category: "productivity"
    requires:
      bins:
        - gws
    cliHelp: "gws drive +upload --help"
---

# drive +upload

> **PREREQUISITE:** Read `../gws-shared/SKILL.md` for auth, global flags, and security rules.

Upload a local file to Google Drive. The uploaded file is **private** until you grant access.

## Usage

```bash
gws drive +upload <FILE> [--name <TEXT>] [--parent <FOLDER_ID>]
```

## Tips

- **Sharing:** After upload, if anyone else will open or receive the link, ALWAYS grant them access with `gws drive permissions create` for each named person / email (see `gws-drive`). Do not share the URL while it still requires "Request access."
- When the upload will be emailed, grant every message recipient (`--to` / `--cc`) before sending.

> [!CAUTION]
> This is a **write** command — confirm with the user before executing.

## See Also

- [gws-drive](../gws-drive/SKILL.md) — Permissions and create
- [gws-shared](../gws-shared/SKILL.md) — Global flags and auth
