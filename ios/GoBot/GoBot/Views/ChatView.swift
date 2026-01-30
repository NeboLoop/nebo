import SwiftUI

struct ChatView: View {
    @EnvironmentObject var authService: AuthService
    @StateObject private var webSocket = WebSocketManager()
    @State private var messages: [Message] = []
    @State private var inputText = ""
    @State private var isStreaming = false
    @FocusState private var isInputFocused: Bool

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                // Messages list
                ScrollViewReader { proxy in
                    ScrollView {
                        LazyVStack(spacing: 12) {
                            ForEach(messages) { message in
                                MessageBubble(message: message)
                                    .id(message.id)
                            }

                            // Streaming indicator
                            if isStreaming && !webSocket.streamingContent.isEmpty {
                                MessageBubble(
                                    message: Message(
                                        role: .assistant,
                                        content: webSocket.streamingContent,
                                        status: .sending
                                    )
                                )
                                .id("streaming")
                            }
                        }
                        .padding()
                    }
                    .onChange(of: messages.count) {
                        withAnimation {
                            proxy.scrollTo(messages.last?.id, anchor: .bottom)
                        }
                    }
                    .onChange(of: webSocket.streamingContent) {
                        if isStreaming {
                            withAnimation {
                                proxy.scrollTo("streaming", anchor: .bottom)
                            }
                        }
                    }
                }

                Divider()

                // Input area
                HStack(spacing: 12) {
                    TextField("Message", text: $inputText, axis: .vertical)
                        .textFieldStyle(.plain)
                        .lineLimit(1...5)
                        .focused($isInputFocused)
                        .onSubmit {
                            sendMessage()
                        }

                    Button {
                        sendMessage()
                    } label: {
                        Image(systemName: "arrow.up.circle.fill")
                            .font(.title2)
                    }
                    .disabled(inputText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                }
                .padding()
                .background(.background)
            }
            .navigationTitle("GoBot")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Circle()
                        .fill(webSocket.isConnected ? .green : .red)
                        .frame(width: 8, height: 8)
                }

                ToolbarItem(placement: .topBarTrailing) {
                    Menu {
                        Button("Settings") {
                            // TODO: Navigate to settings
                        }
                        Button("Logout", role: .destructive) {
                            webSocket.disconnect()
                            authService.logout()
                        }
                    } label: {
                        Image(systemName: "ellipsis.circle")
                    }
                }
            }
        }
        .onAppear {
            connectWebSocket()
        }
        .onDisappear {
            webSocket.disconnect()
        }
        .onChange(of: webSocket.lastMessage) {
            handleWebSocketMessage()
        }
    }

    private func connectWebSocket() {
        Task {
            if let token = await getAccessToken() {
                webSocket.setAccessToken(token)
            }
            webSocket.connect()
        }
    }

    private func getAccessToken() async -> String? {
        // Access token is stored in keychain - get it from auth service
        // For now, we rely on the WebSocket manager having it set during login flow
        return nil
    }

    private func sendMessage() {
        let text = inputText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }

        let message = Message(role: .user, content: text, status: .sent)
        messages.append(message)
        inputText = ""

        isStreaming = true
        webSocket.streamingContent = ""
        webSocket.send(text)
    }

    private func handleWebSocketMessage() {
        guard let message = webSocket.lastMessage else { return }

        switch message {
        case .text(let content):
            // Complete message received
            isStreaming = false
            let assistantMessage = Message(role: .assistant, content: content, status: .sent)
            messages.append(assistantMessage)
            webSocket.streamingContent = ""

        case .error(let error):
            isStreaming = false
            let errorMessage = Message(role: .assistant, content: "Error: \(error)", status: .failed)
            messages.append(errorMessage)

        case .disconnected:
            // Could show reconnection UI
            break

        case .connected:
            // Connected successfully
            break

        case .chunk:
            // Handled by streamingContent
            break
        }
    }
}

struct MessageBubble: View {
    let message: Message

    var isUser: Bool {
        message.role == .user
    }

    var body: some View {
        HStack {
            if isUser { Spacer(minLength: 60) }

            VStack(alignment: isUser ? .trailing : .leading, spacing: 4) {
                Text(message.content)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                    .background(isUser ? Color.accentColor : Color(.systemGray5))
                    .foregroundStyle(isUser ? .white : .primary)
                    .clipShape(RoundedRectangle(cornerRadius: 16))

                if message.status == .sending {
                    ProgressView()
                        .scaleEffect(0.6)
                }
            }

            if !isUser { Spacer(minLength: 60) }
        }
    }
}

#Preview {
    ChatView()
        .environmentObject(AuthService())
}
