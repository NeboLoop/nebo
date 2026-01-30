package com.gobot.app

import com.google.gson.Gson
import com.google.gson.annotations.SerializedName
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import java.util.concurrent.TimeUnit

sealed class ConnectionState {
    object Disconnected : ConnectionState()
    object Connecting : ConnectionState()
    object Connected : ConnectionState()
    data class Error(val message: String) : ConnectionState()
}

data class WebSocketMessage(
    val type: String,
    val content: String? = null,
    val data: Map<String, Any>? = null,
    @SerializedName("message_id") val messageId: String? = null
)

data class AgentMessage(
    val id: String,
    val content: String,
    val role: String,
    val timestamp: Long = System.currentTimeMillis(),
    val isStreaming: Boolean = false
)

class WebSocketManager(private val authManager: AuthManager) {

    private val gson = Gson()
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)

    private var webSocket: WebSocket? = null
    private var reconnectAttempts = 0
    private val maxReconnectAttempts = 5

    private val _connectionState = MutableStateFlow<ConnectionState>(ConnectionState.Disconnected)
    val connectionState: StateFlow<ConnectionState> = _connectionState

    private val _messages = MutableSharedFlow<AgentMessage>(replay = 0, extraBufferCapacity = 100)
    val messages: SharedFlow<AgentMessage> = _messages

    private val _streamingContent = MutableStateFlow<String?>(null)
    val streamingContent: StateFlow<String?> = _streamingContent

    private val client = OkHttpClient.Builder()
        .connectTimeout(30, TimeUnit.SECONDS)
        .readTimeout(0, TimeUnit.SECONDS) // No timeout for WebSocket
        .writeTimeout(30, TimeUnit.SECONDS)
        .pingInterval(30, TimeUnit.SECONDS)
        .build()

    private fun getWebSocketUrl(): String {
        val baseUrl = authManager.getServerUrl()
            .replace("http://", "ws://")
            .replace("https://", "wss://")
        return "$baseUrl/ws/agent"
    }

    fun connect() {
        if (_connectionState.value == ConnectionState.Connected ||
            _connectionState.value == ConnectionState.Connecting) {
            return
        }

        val token = authManager.getToken() ?: run {
            _connectionState.value = ConnectionState.Error("Not authenticated")
            return
        }

        _connectionState.value = ConnectionState.Connecting

        val request = Request.Builder()
            .url(getWebSocketUrl())
            .addHeader("Authorization", "Bearer $token")
            .build()

        webSocket = client.newWebSocket(request, object : WebSocketListener() {
            override fun onOpen(webSocket: WebSocket, response: Response) {
                _connectionState.value = ConnectionState.Connected
                reconnectAttempts = 0

                // Send initial connection message
                val connectMsg = WebSocketMessage(type = "connect", content = "android")
                webSocket.send(gson.toJson(connectMsg))
            }

            override fun onMessage(webSocket: WebSocket, text: String) {
                handleMessage(text)
            }

            override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
                webSocket.close(1000, null)
                _connectionState.value = ConnectionState.Disconnected
            }

            override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
                _connectionState.value = ConnectionState.Disconnected
                attemptReconnect()
            }

            override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
                _connectionState.value = ConnectionState.Error(t.message ?: "Connection failed")
                attemptReconnect()
            }
        })
    }

    private fun handleMessage(text: String) {
        try {
            val message = gson.fromJson(text, WebSocketMessage::class.java)

            when (message.type) {
                "message", "response" -> {
                    message.content?.let { content ->
                        val agentMessage = AgentMessage(
                            id = message.messageId ?: System.currentTimeMillis().toString(),
                            content = content,
                            role = "assistant"
                        )
                        scope.launch {
                            _messages.emit(agentMessage)
                        }
                    }
                }

                "stream_start" -> {
                    _streamingContent.value = ""
                }

                "stream_delta" -> {
                    message.content?.let { delta ->
                        _streamingContent.value = (_streamingContent.value ?: "") + delta
                    }
                }

                "stream_end" -> {
                    _streamingContent.value?.let { content ->
                        val agentMessage = AgentMessage(
                            id = message.messageId ?: System.currentTimeMillis().toString(),
                            content = content,
                            role = "assistant"
                        )
                        scope.launch {
                            _messages.emit(agentMessage)
                        }
                    }
                    _streamingContent.value = null
                }

                "error" -> {
                    scope.launch {
                        _messages.emit(AgentMessage(
                            id = System.currentTimeMillis().toString(),
                            content = "Error: ${message.content ?: "Unknown error"}",
                            role = "system"
                        ))
                    }
                }

                "ping" -> {
                    webSocket?.send(gson.toJson(WebSocketMessage(type = "pong")))
                }
            }
        } catch (e: Exception) {
            e.printStackTrace()
        }
    }

    fun sendMessage(content: String) {
        val message = WebSocketMessage(
            type = "message",
            content = content
        )
        webSocket?.send(gson.toJson(message))

        // Emit user message locally
        scope.launch {
            _messages.emit(AgentMessage(
                id = System.currentTimeMillis().toString(),
                content = content,
                role = "user"
            ))
        }
    }

    private fun attemptReconnect() {
        if (reconnectAttempts >= maxReconnectAttempts) {
            _connectionState.value = ConnectionState.Error("Max reconnection attempts reached")
            return
        }

        reconnectAttempts++
        scope.launch {
            delay(2000L * reconnectAttempts) // Exponential backoff
            connect()
        }
    }

    fun disconnect() {
        webSocket?.close(1000, "User disconnected")
        webSocket = null
        _connectionState.value = ConnectionState.Disconnected
    }
}
