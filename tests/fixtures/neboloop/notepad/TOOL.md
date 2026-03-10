# Notepad

A simple note storage tool. Use it to save, read, and list text notes that persist across workflow activities.

## Actions

### save
Save a note with a key.
- `key` (required): Short identifier for the note
- `content` (required): Text content to save

### read
Read a saved note by key.
- `key` (required): The note key to read

### list
List all saved note keys. No parameters required.

## Example Usage

Save research findings:
```
notepad(action: "save", key: "company-funding", content: "Series B, $45M, led by Sequoia")
```

Read them back:
```
notepad(action: "read", key: "company-funding")
```

List all notes:
```
notepad(action: "list")
```
