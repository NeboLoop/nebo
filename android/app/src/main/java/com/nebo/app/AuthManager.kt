package com.nebo.app

import android.content.Context
import android.content.SharedPreferences
import androidx.security.crypto.EncryptedSharedPreferences
import androidx.security.crypto.MasterKey
import com.google.gson.Gson
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow

class AuthManager(context: Context) {

    private val gson = Gson()

    // Encrypted storage for sensitive data
    private val masterKey = MasterKey.Builder(context)
        .setKeyScheme(MasterKey.KeyScheme.AES256_GCM)
        .build()

    private val securePrefs: SharedPreferences = EncryptedSharedPreferences.create(
        context,
        "nebo_secure_prefs",
        masterKey,
        EncryptedSharedPreferences.PrefKeyEncryptionScheme.AES256_SIV,
        EncryptedSharedPreferences.PrefValueEncryptionScheme.AES256_GCM
    )

    // Regular prefs for non-sensitive settings
    private val prefs: SharedPreferences = context.getSharedPreferences(
        "nebo_prefs",
        Context.MODE_PRIVATE
    )

    private val _isLoggedIn = MutableStateFlow(getToken() != null)
    val isLoggedIn: StateFlow<Boolean> = _isLoggedIn

    private val _currentUser = MutableStateFlow<User?>(null)
    val currentUser: StateFlow<User?> = _currentUser

    init {
        // Load user on init
        getUserJson()?.let { json ->
            try {
                _currentUser.value = gson.fromJson(json, User::class.java)
            } catch (e: Exception) {
                // Ignore parse errors
            }
        }
    }

    fun getToken(): String? {
        return securePrefs.getString(KEY_TOKEN, null)
    }

    fun saveToken(token: String) {
        securePrefs.edit().putString(KEY_TOKEN, token).apply()
        _isLoggedIn.value = true
    }

    fun saveUser(user: User) {
        val json = gson.toJson(user)
        prefs.edit().putString(KEY_USER, json).apply()
        _currentUser.value = user
    }

    private fun getUserJson(): String? {
        return prefs.getString(KEY_USER, null)
    }

    fun clearAuth() {
        securePrefs.edit().remove(KEY_TOKEN).apply()
        prefs.edit().remove(KEY_USER).apply()
        _isLoggedIn.value = false
        _currentUser.value = null
    }

    // Server URL management
    fun getServerUrl(): String {
        return prefs.getString(KEY_SERVER_URL, DEFAULT_SERVER_URL) ?: DEFAULT_SERVER_URL
    }

    fun setServerUrl(url: String) {
        prefs.edit().putString(KEY_SERVER_URL, url.trimEnd('/')).apply()
    }

    // Gateway connection (for remote access)
    fun getGatewayUrl(): String? {
        return prefs.getString(KEY_GATEWAY_URL, null)
    }

    fun setGatewayUrl(url: String?) {
        if (url != null) {
            prefs.edit().putString(KEY_GATEWAY_URL, url.trimEnd('/')).apply()
        } else {
            prefs.edit().remove(KEY_GATEWAY_URL).apply()
        }
    }

    fun getGatewayToken(): String? {
        return securePrefs.getString(KEY_GATEWAY_TOKEN, null)
    }

    fun setGatewayToken(token: String?) {
        if (token != null) {
            securePrefs.edit().putString(KEY_GATEWAY_TOKEN, token).apply()
        } else {
            securePrefs.edit().remove(KEY_GATEWAY_TOKEN).apply()
        }
    }

    fun isUsingGateway(): Boolean {
        return prefs.getBoolean(KEY_USE_GATEWAY, false)
    }

    fun setUseGateway(use: Boolean) {
        prefs.edit().putBoolean(KEY_USE_GATEWAY, use).apply()
    }

    companion object {
        private const val KEY_TOKEN = "auth_token"
        private const val KEY_USER = "current_user"
        private const val KEY_SERVER_URL = "server_url"
        private const val KEY_GATEWAY_URL = "gateway_url"
        private const val KEY_GATEWAY_TOKEN = "gateway_token"
        private const val KEY_USE_GATEWAY = "use_gateway"

        // Default to localhost for development
        // Users will configure their own server URL or gateway
        private const val DEFAULT_SERVER_URL = "http://10.0.2.2:29875" // Android emulator localhost
    }
}
