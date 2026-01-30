import Foundation
import Security

@MainActor
class AuthService: ObservableObject {
    @Published var isAuthenticated = false
    @Published var currentUser: User?
    @Published var isLoading = false
    @Published var error: String?

    private let keychainService = "com.gobot.ios"
    private let accessTokenKey = "accessToken"
    private let refreshTokenKey = "refreshToken"

    init() {
        // Check for existing token on launch
        if let token = getKeychainItem(key: accessTokenKey) {
            Task {
                await APIClient.shared.setAccessToken(token)
                isAuthenticated = true
                await loadUserProfile()
            }
        }
    }

    func login(email: String, password: String) async {
        isLoading = true
        error = nil

        do {
            let response = try await APIClient.shared.login(email: email, password: password)

            // Store tokens
            setKeychainItem(key: accessTokenKey, value: response.accessToken)
            if let refreshToken = response.refreshToken {
                setKeychainItem(key: refreshTokenKey, value: refreshToken)
            }

            // Update API client
            await APIClient.shared.setAccessToken(response.accessToken)

            // Update state
            currentUser = response.user
            isAuthenticated = true

        } catch let apiError as APIError {
            error = apiError.errorDescription
        } catch {
            self.error = error.localizedDescription
        }

        isLoading = false
    }

    func logout() {
        deleteKeychainItem(key: accessTokenKey)
        deleteKeychainItem(key: refreshTokenKey)

        Task {
            await APIClient.shared.setAccessToken(nil)
        }

        currentUser = nil
        isAuthenticated = false
    }

    func refreshTokenIfNeeded() async {
        guard let refreshToken = getKeychainItem(key: refreshTokenKey) else {
            logout()
            return
        }

        do {
            let response = try await APIClient.shared.refreshToken(refreshToken)

            setKeychainItem(key: accessTokenKey, value: response.accessToken)
            if let newRefreshToken = response.refreshToken {
                setKeychainItem(key: refreshTokenKey, value: newRefreshToken)
            }

            await APIClient.shared.setAccessToken(response.accessToken)
            currentUser = response.user

        } catch {
            // Refresh failed - force logout
            logout()
        }
    }

    private func loadUserProfile() async {
        do {
            currentUser = try await APIClient.shared.getUserProfile()
        } catch APIError.unauthorized {
            await refreshTokenIfNeeded()
        } catch {
            print("Failed to load user profile: \(error)")
        }
    }

    // MARK: - Keychain Helpers

    private func setKeychainItem(key: String, value: String) {
        let data = value.data(using: .utf8)!

        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: keychainService,
            kSecAttrAccount as String: key,
            kSecValueData as String: data
        ]

        SecItemDelete(query as CFDictionary)
        SecItemAdd(query as CFDictionary, nil)
    }

    private func getKeychainItem(key: String) -> String? {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: keychainService,
            kSecAttrAccount as String: key,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne
        ]

        var dataTypeRef: AnyObject?
        let status = SecItemCopyMatching(query as CFDictionary, &dataTypeRef)

        guard status == errSecSuccess,
              let data = dataTypeRef as? Data,
              let value = String(data: data, encoding: .utf8) else {
            return nil
        }

        return value
    }

    private func deleteKeychainItem(key: String) {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: keychainService,
            kSecAttrAccount as String: key
        ]

        SecItemDelete(query as CFDictionary)
    }
}
