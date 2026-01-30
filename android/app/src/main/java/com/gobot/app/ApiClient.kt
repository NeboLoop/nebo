package com.gobot.app

import com.google.gson.Gson
import com.google.gson.annotations.SerializedName
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import java.util.concurrent.TimeUnit

// API Data Classes
data class LoginRequest(
    val email: String,
    val password: String
)

data class LoginResponse(
    val token: String,
    val user: User
)

data class User(
    val id: String,
    val email: String,
    val name: String
)

data class ChatRequest(
    val message: String,
    val channel: String = "android"
)

data class ChatResponse(
    val id: String,
    val content: String,
    val role: String,
    @SerializedName("created_at") val createdAt: String
)

data class ApiError(
    val error: String,
    val code: Int
)

class ApiClient(private val authManager: AuthManager) {

    private val gson = Gson()
    private val jsonMediaType = "application/json; charset=utf-8".toMediaType()

    private val client = OkHttpClient.Builder()
        .connectTimeout(30, TimeUnit.SECONDS)
        .readTimeout(60, TimeUnit.SECONDS)
        .writeTimeout(60, TimeUnit.SECONDS)
        .addInterceptor { chain ->
            val originalRequest = chain.request()
            val token = authManager.getToken()

            val request = if (token != null) {
                originalRequest.newBuilder()
                    .header("Authorization", "Bearer $token")
                    .build()
            } else {
                originalRequest
            }

            chain.proceed(request)
        }
        .build()

    // Get base URL from settings
    private fun getBaseUrl(): String {
        return authManager.getServerUrl()
    }

    suspend fun login(email: String, password: String): Result<LoginResponse> = withContext(Dispatchers.IO) {
        try {
            val body = gson.toJson(LoginRequest(email, password))
                .toRequestBody(jsonMediaType)

            val request = Request.Builder()
                .url("${getBaseUrl()}/api/v1/auth/login")
                .post(body)
                .build()

            client.newCall(request).execute().use { response ->
                val responseBody = response.body?.string() ?: ""

                if (response.isSuccessful) {
                    val loginResponse = gson.fromJson(responseBody, LoginResponse::class.java)
                    authManager.saveToken(loginResponse.token)
                    authManager.saveUser(loginResponse.user)
                    Result.success(loginResponse)
                } else {
                    val error = try {
                        gson.fromJson(responseBody, ApiError::class.java).error
                    } catch (e: Exception) {
                        "Login failed: ${response.code}"
                    }
                    Result.failure(Exception(error))
                }
            }
        } catch (e: Exception) {
            Result.failure(e)
        }
    }

    suspend fun sendMessage(message: String): Result<ChatResponse> = withContext(Dispatchers.IO) {
        try {
            val body = gson.toJson(ChatRequest(message))
                .toRequestBody(jsonMediaType)

            val request = Request.Builder()
                .url("${getBaseUrl()}/api/v1/agent/chat")
                .post(body)
                .build()

            client.newCall(request).execute().use { response ->
                val responseBody = response.body?.string() ?: ""

                if (response.isSuccessful) {
                    val chatResponse = gson.fromJson(responseBody, ChatResponse::class.java)
                    Result.success(chatResponse)
                } else {
                    val error = try {
                        gson.fromJson(responseBody, ApiError::class.java).error
                    } catch (e: Exception) {
                        "Failed to send message: ${response.code}"
                    }
                    Result.failure(Exception(error))
                }
            }
        } catch (e: Exception) {
            Result.failure(e)
        }
    }

    suspend fun getHistory(): Result<List<ChatResponse>> = withContext(Dispatchers.IO) {
        try {
            val request = Request.Builder()
                .url("${getBaseUrl()}/api/v1/agent/history")
                .get()
                .build()

            client.newCall(request).execute().use { response ->
                val responseBody = response.body?.string() ?: "[]"

                if (response.isSuccessful) {
                    val history = gson.fromJson(responseBody, Array<ChatResponse>::class.java).toList()
                    Result.success(history)
                } else {
                    Result.failure(Exception("Failed to get history: ${response.code}"))
                }
            }
        } catch (e: Exception) {
            Result.failure(e)
        }
    }

    fun logout() {
        authManager.clearAuth()
    }
}
