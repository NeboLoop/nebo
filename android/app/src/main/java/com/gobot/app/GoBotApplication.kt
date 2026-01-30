package com.gobot.app

import android.app.Application

class GoBotApplication : Application() {

    lateinit var apiClient: ApiClient
        private set

    lateinit var webSocketManager: WebSocketManager
        private set

    lateinit var authManager: AuthManager
        private set

    override fun onCreate() {
        super.onCreate()
        instance = this

        authManager = AuthManager(this)
        apiClient = ApiClient(authManager)
        webSocketManager = WebSocketManager(authManager)
    }

    companion object {
        lateinit var instance: GoBotApplication
            private set
    }
}
