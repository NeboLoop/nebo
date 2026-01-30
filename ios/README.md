# GoBot iOS App

A native SwiftUI companion app for chatting with GoBot from your iPhone.

## Architecture

```
GoBot/
├── GoBotApp.swift           # App entry point
├── Models/
│   ├── Message.swift        # Chat message model
│   ├── Conversation.swift   # Conversation model
│   └── User.swift           # User profile model
├── Views/
│   ├── RootView.swift       # Navigation root (login vs chat)
│   ├── LoginView.swift      # Login screen
│   └── ChatView.swift       # Main chat interface
└── Services/
    ├── APIClient.swift      # HTTP API calls
    ├── AuthService.swift    # Authentication + Keychain
    └── WebSocketManager.swift # Real-time messaging
```

## Prerequisites

- Xcode 15+ (for iOS 17+ SDK)
- GoBot backend running locally or deployed
- A GoBot user account

## Local Development Setup

### 1. Start the GoBot backend

```bash
cd /path/to/gobot
make air  # Backend with hot reload
```

Backend runs at `http://localhost:29875`

### 2. Open in Xcode

```bash
open ios/GoBot/GoBot.xcodeproj
```

### 3. Run on Simulator

1. Select an iPhone simulator (iPhone 15 Pro recommended)
2. Press Cmd+R or click the Play button
3. The app will connect to `http://localhost:29875` by default

### 4. Run on Physical Device (Local Network)

To test on a real iPhone while connecting to your local Mac:

1. **Find your Mac's local IP:**
   ```bash
   ipconfig getifaddr en0
   ```
   (e.g., `192.168.1.100`)

2. **Update APIClient.swift** (line 32):
   ```swift
   private var baseURL: String = "http://192.168.1.100:29875"
   ```

3. **Update WebSocketManager.swift** (line 61):
   ```swift
   private var baseURL: String = "ws://192.168.1.100:29875"
   ```

4. **Add App Transport Security exception** in `Info.plist`:
   ```xml
   <key>NSAppTransportSecurity</key>
   <dict>
       <key>NSAllowsLocalNetworking</key>
       <true/>
   </dict>
   ```

5. Ensure your iPhone and Mac are on the same WiFi network.

## How It Works

### Authentication Flow

1. User enters email/password on LoginView
2. APIClient calls `/api/v1/auth/login`
3. JWT tokens stored securely in iOS Keychain
4. Token attached to all subsequent API/WebSocket requests

### Chat Flow

1. ChatView establishes WebSocket connection to `/ws/agent`
2. User types message → sent via WebSocket as JSON: `{"type":"message","content":"..."}`
3. GoBot responds with:
   - `{"type":"chunk","content":"..."}` - streaming text chunks
   - `{"type":"message","content":"..."}` - complete message
   - `{"type":"error","error":"..."}` - error message

### API Endpoints Used

| Endpoint | Purpose |
|----------|---------|
| `POST /api/v1/auth/login` | Login with email/password |
| `POST /api/v1/auth/refresh` | Refresh expired token |
| `GET /api/v1/user/profile` | Get user profile |
| `GET /api/v1/chat/history` | Load chat history |
| `WS /ws/agent` | Real-time chat via WebSocket |

## Features

- Real-time streaming responses
- Auto-reconnect on connection loss
- Secure token storage in Keychain
- Dark mode support
- Keyboard-aware UI

## Production Deployment

For production use:

1. Update `baseURL` in both services to point to your deployed GoBot instance
2. Ensure HTTPS is configured (required for App Store)
3. Update the bundle identifier and signing
4. Submit to TestFlight or App Store

## Troubleshooting

### "Connection Failed"
- Verify GoBot backend is running
- Check the baseURL matches your backend address
- Ensure firewall allows connections on port 29875

### "Unauthorized"
- Token may have expired - try logging out and back in
- Verify the user account exists in GoBot

### WebSocket Disconnects
- The app auto-reconnects after 3 seconds
- Check network stability
- Verify `/ws/agent` endpoint is available

## Contributing

1. Follow Swift/SwiftUI conventions
2. Use async/await for all async operations
3. Keep UI on MainActor
4. Store secrets only in Keychain, never in UserDefaults
