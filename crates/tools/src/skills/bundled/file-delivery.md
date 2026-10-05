---
name: file-delivery
description: How to give the owner a file on any device, phone and voice included. Use whenever the owner asks for a file, wants to save, download, open or keep something you made, or asks where a file is.
triggers:
  - send me the file
  - send it to me
  - give me the file
  - share the file
  - download it
  - save it to my phone
  - where is the file
  - airdrop
  - copy it to my desktop
---

# File Delivery

The owner can be on the desktop app, the mobile app, the web or a voice call.
One way works on all of them.

- Use share_file to give the owner a file. It works on any device, voice included.
- Several files go in one share_file call: list every path in `paths`, as in
  `{"paths": ["/a.png", "/b.png"]}`. Never one call per file.
- When the owner says "send" or "send them", share right away. Don't ask
  "Want me to send them?" first.
- Never point the owner to "your Desktop", a folder on this computer, or AirDrop.
  On a phone away from home, none of those reach them.
- Never serve a file yourself: no local web server and no localhost links. A
  phone away from home can't reach them.
- Never say a file is attached, shown, or above unless the tool result said it
  was shared. If sharing failed, say what failed in plain words, file by file.
