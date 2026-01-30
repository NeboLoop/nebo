import Foundation

enum WebSocketMessage: Codable {
    case text(String)
    case chunk(String)
    case error(String)
    case connected
    case disconnected

    enum CodingKeys: String, CodingKey {
        case type
        case content
        case error
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let type = try container.decode(String.self, forKey: .type)

        switch type {
        case "message":
            let content = try container.decode(String.self, forKey: .content)
            self = .text(content)
        case "chunk":
            let content = try container.decode(String.self, forKey: .content)
            self = .chunk(content)
        case "error":
            let error = try container.decode(String.self, forKey: .error)
            self = .error(error)
        default:
            self = .text("")
        }
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .text(let content):
            try container.encode("message", forKey: .type)
            try container.encode(content, forKey: .content)
        case .chunk(let content):
            try container.encode("chunk", forKey: .type)
            try container.encode(content, forKey: .content)
        case .error(let error):
            try container.encode("error", forKey: .type)
            try container.encode(error, forKey: .error)
        case .connected, .disconnected:
            break
        }
    }
}

@MainActor
class WebSocketManager: ObservableObject {
    @Published var isConnected = false
    @Published var lastMessage: WebSocketMessage?
    @Published var streamingContent: String = ""

    private var webSocketTask: URLSessionWebSocketTask?
    private var pingTimer: Timer?
    private var baseURL: String = "ws://localhost:29875"
    private var accessToken: String?

    func setBaseURL(_ url: String) {
        var wsURL = url.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        wsURL = wsURL.replacingOccurrences(of: "https://", with: "wss://")
        wsURL = wsURL.replacingOccurrences(of: "http://", with: "ws://")
        baseURL = wsURL
    }

    func setAccessToken(_ token: String?) {
        accessToken = token
    }

    func connect() {
        guard let url = URL(string: "\(baseURL)/ws/agent") else {
            print("Invalid WebSocket URL")
            return
        }

        var request = URLRequest(url: url)
        if let token = accessToken {
            request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }

        webSocketTask = URLSession.shared.webSocketTask(with: request)
        webSocketTask?.resume()

        isConnected = true
        lastMessage = .connected
        receiveMessage()
        startPingTimer()
    }

    func disconnect() {
        stopPingTimer()
        webSocketTask?.cancel(with: .goingAway, reason: nil)
        webSocketTask = nil
        isConnected = false
        lastMessage = .disconnected
    }

    func send(_ message: String) {
        struct SendMessage: Codable {
            let type: String
            let content: String
        }

        let msg = SendMessage(type: "message", content: message)
        guard let data = try? JSONEncoder().encode(msg),
              let jsonString = String(data: data, encoding: .utf8) else {
            return
        }

        webSocketTask?.send(.string(jsonString)) { [weak self] error in
            if let error = error {
                print("WebSocket send error: \(error)")
                Task { @MainActor in
                    self?.handleDisconnection()
                }
            }
        }
    }

    private func receiveMessage() {
        webSocketTask?.receive { [weak self] result in
            Task { @MainActor in
                switch result {
                case .success(let message):
                    switch message {
                    case .string(let text):
                        self?.handleIncomingMessage(text)
                    case .data(let data):
                        if let text = String(data: data, encoding: .utf8) {
                            self?.handleIncomingMessage(text)
                        }
                    @unknown default:
                        break
                    }
                    self?.receiveMessage()

                case .failure(let error):
                    print("WebSocket receive error: \(error)")
                    self?.handleDisconnection()
                }
            }
        }
    }

    private func handleIncomingMessage(_ text: String) {
        guard let data = text.data(using: .utf8) else { return }

        do {
            let message = try JSONDecoder().decode(WebSocketMessage.self, from: data)

            switch message {
            case .chunk(let content):
                streamingContent += content
            case .text(let content):
                streamingContent = ""
                lastMessage = .text(content)
            case .error(let error):
                lastMessage = .error(error)
            default:
                lastMessage = message
            }
        } catch {
            print("Failed to decode WebSocket message: \(error)")
        }
    }

    private func handleDisconnection() {
        isConnected = false
        lastMessage = .disconnected
        stopPingTimer()

        // Auto-reconnect after 3 seconds
        Task {
            try? await Task.sleep(nanoseconds: 3_000_000_000)
            if !isConnected {
                connect()
            }
        }
    }

    private func startPingTimer() {
        pingTimer = Timer.scheduledTimer(withTimeInterval: 30, repeats: true) { [weak self] _ in
            self?.webSocketTask?.sendPing { error in
                if let error = error {
                    print("Ping error: \(error)")
                }
            }
        }
    }

    private func stopPingTimer() {
        pingTimer?.invalidate()
        pingTimer = nil
    }
}
