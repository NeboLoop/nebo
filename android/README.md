# GoBot Android App

Native Android companion app for GoBot - your AI assistant.

## Features

- Real-time chat with GoBot via WebSocket
- Streaming message support
- Secure token storage (EncryptedSharedPreferences)
- Gateway support for remote access
- Material 3 / Material You design
- Dark mode support

## Requirements

- Android Studio Hedgehog (2023.1.1) or newer
- Android SDK 34
- Kotlin 1.9+
- Minimum Android 8.0 (API 26)

## Setup

### 1. Open in Android Studio

```bash
# Open the android/ directory in Android Studio
```

### 2. Configure Server URL

The app defaults to `http://10.0.2.2:29875` (Android emulator's localhost alias).

**For physical device on same network:**
1. Get your computer's IP: `ipconfig getifaddr en0` (macOS)
2. Open Settings in the app
3. Set "Local Server" to `http://YOUR_IP:29875`

### 3. Build and Run

```bash
# Via Android Studio: Run > Run 'app'
# Or via command line:
./gradlew installDebug
```

## Project Structure

```
android/
├── app/src/main/java/com/gobot/app/
│   ├── GoBotApplication.kt     # App entry, dependency holder
│   ├── MainActivity.kt         # Navigation host
│   ├── ApiClient.kt            # HTTP API calls
│   ├── WebSocketManager.kt     # Real-time messaging
│   ├── AuthManager.kt          # Token & settings storage
│   └── ui/
│       ├── LoginScreen.kt      # Authentication UI
│       ├── ChatScreen.kt       # Main chat interface
│       ├── SettingsScreen.kt   # Connection settings
│       └── theme/              # Material 3 theming
└── build.gradle.kts            # Dependencies
```

## Gateway Connection

For secure remote access (outside local network):

1. Set up the GoBot Gateway on a public server
2. In Settings, enable "Use Gateway"
3. Enter your Gateway URL and access token
4. The app will route all traffic through the gateway

## Architecture

- **UI**: Jetpack Compose with Material 3
- **Navigation**: Compose Navigation
- **Networking**: OkHttp + Retrofit
- **WebSocket**: OkHttp WebSocket
- **Security**: AndroidX Security Crypto (encrypted prefs)
- **Async**: Kotlin Coroutines + Flow

## API Endpoints Used

| Endpoint | Method | Purpose |
|----------|--------|---------|
| `/api/v1/auth/login` | POST | User authentication |
| `/api/v1/agent/chat` | POST | Send message (fallback) |
| `/api/v1/agent/history` | GET | Load chat history |
| `/ws/agent` | WS | Real-time bidirectional chat |

## Building for Release

```bash
# Create signed APK
./gradlew assembleRelease

# Create signed AAB (for Play Store)
./gradlew bundleRelease
```

Add signing config in `app/build.gradle.kts`:

```kotlin
signingConfigs {
    create("release") {
        storeFile = file("keystore.jks")
        storePassword = "..."
        keyAlias = "gobot"
        keyPassword = "..."
    }
}
```

## Troubleshooting

### "Connection failed"
- Check server is running: `curl http://YOUR_IP:29875/health`
- Ensure phone is on same WiFi network
- Check firewall allows port 29875

### "Not authenticated"
- Token may have expired, try logging in again
- Clear app data and re-authenticate

### WebSocket keeps disconnecting
- Check network stability
- Server may be restarting (hot reload)
