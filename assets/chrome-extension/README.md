# Nebo Browser Relay Extension

Chrome extension that connects your existing Chrome tabs to Nebo's browser relay, allowing the agent to control pages with all your authenticated sessions intact.

## Development

```bash
# Install dependencies
pnpm install

# Build extension (outputs to dist/)
pnpm run build

# Watch mode for development
pnpm run watch
```

## Installation

### From Source (Development)

1. Run `pnpm run build` to create the `dist/` folder
2. Open Chrome and navigate to `chrome://extensions`
3. Enable "Developer mode" (toggle in top right)
4. Click "Load unpacked"
5. Select the `dist/` folder

### Pre-built

Load the `dist/` folder directly if already built.

## Usage

1. Start Nebo:
   ```bash
   nebo agent
   ```

2. Click the Nebo extension icon on any tab to attach it

3. Badge states:
   - **ON** (orange): Tab attached and controlled by Nebo
   - **…** (yellow): Connecting to relay
   - **!** (red): Relay not reachable

4. Click again to detach

## Architecture

```
┌─────────────────┐     WebSocket      ┌─────────────────┐
│  Nebo Agent     │◄──────────────────►│  Extension      │
│  (Playwright)   │   CDP Messages     │  Relay Server   │
└─────────────────┘                    └────────┬────────┘
                                                │
                                       Chrome Debugger API
                                                │
                                       ┌────────▼────────┐
                                       │  Your Chrome    │
                                       │  Tabs           │
                                       └─────────────────┘
```

The extension:
1. Connects to Nebo's relay server at `ws://127.0.0.1:9224/extension`
2. Uses Chrome's `chrome.debugger` API to control attached tabs
3. Forwards CDP (Chrome DevTools Protocol) messages between Nebo and Chrome

## Configuration

Click the extension icon → right-click → "Options" to change the relay port.

Default port: `9224`

## Security

- Extension only connects to localhost (127.0.0.1)
- You control which tabs are attached
- Nebo can only interact with tabs you explicitly attach
- Detach tabs when not in use

## Agent Commands

Once a tab is attached:

```
web(action: navigate, url: "https://gmail.com", profile: "chrome")
web(action: snapshot)  # Returns aria tree with refs like [e1], [e2]
web(action: click, ref: "e5")
web(action: fill, ref: "e3", value: "search query")
web(action: type, ref: "e3", text: "hello")
web(action: screenshot)
```

The `profile: "chrome"` routes commands through this extension relay instead of Nebo's managed browser.

## Project Structure

```
assets/chrome-extension/
├── src/                    # TypeScript source
│   ├── types.ts           # Type definitions
│   ├── storage.ts         # Chrome storage helpers
│   ├── badge.ts           # Badge UI state
│   ├── tabs.ts            # Tab management
│   ├── relay.ts           # WebSocket connection
│   ├── background.ts      # Service worker entry
│   └── options.ts         # Options page
├── icons/                  # Extension icons
├── manifest.json          # Chrome manifest
├── options.html           # Options page HTML
├── package.json
├── tsconfig.json
└── dist/                  # Built extension (load this in Chrome)
```
