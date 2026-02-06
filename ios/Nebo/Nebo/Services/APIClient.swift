import Foundation

enum APIError: Error, LocalizedError {
    case invalidURL
    case noData
    case decodingError(Error)
    case serverError(Int, String?)
    case networkError(Error)
    case unauthorized

    var errorDescription: String? {
        switch self {
        case .invalidURL:
            return "Invalid URL"
        case .noData:
            return "No data received"
        case .decodingError(let error):
            return "Decoding error: \(error.localizedDescription)"
        case .serverError(let code, let message):
            return "Server error \(code): \(message ?? "Unknown")"
        case .networkError(let error):
            return "Network error: \(error.localizedDescription)"
        case .unauthorized:
            return "Unauthorized - please log in again"
        }
    }
}

actor APIClient {
    static let shared = APIClient()

    private var baseURL: String = "http://localhost:29875"
    private var accessToken: String?

    func setBaseURL(_ url: String) {
        baseURL = url.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
    }

    func setAccessToken(_ token: String?) {
        accessToken = token
    }

    private func makeRequest<T: Decodable>(
        endpoint: String,
        method: String = "GET",
        body: Encodable? = nil
    ) async throws -> T {
        guard let url = URL(string: "\(baseURL)\(endpoint)") else {
            throw APIError.invalidURL
        }

        var request = URLRequest(url: url)
        request.httpMethod = method
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")

        if let token = accessToken {
            request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }

        if let body = body {
            let encoder = JSONEncoder()
            request.httpBody = try encoder.encode(body)
        }

        let (data, response) = try await URLSession.shared.data(for: request)

        guard let httpResponse = response as? HTTPURLResponse else {
            throw APIError.noData
        }

        if httpResponse.statusCode == 401 {
            throw APIError.unauthorized
        }

        guard (200...299).contains(httpResponse.statusCode) else {
            let message = String(data: data, encoding: .utf8)
            throw APIError.serverError(httpResponse.statusCode, message)
        }

        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601

        do {
            return try decoder.decode(T.self, from: data)
        } catch {
            throw APIError.decodingError(error)
        }
    }

    // MARK: - Auth Endpoints

    func login(email: String, password: String) async throws -> LoginResponse {
        let body = LoginRequest(email: email, password: password)
        return try await makeRequest(endpoint: "/api/v1/auth/login", method: "POST", body: body)
    }

    func refreshToken(_ refreshToken: String) async throws -> LoginResponse {
        struct RefreshRequest: Codable {
            let refreshToken: String
        }
        let body = RefreshRequest(refreshToken: refreshToken)
        return try await makeRequest(endpoint: "/api/v1/auth/refresh", method: "POST", body: body)
    }

    func getUserProfile() async throws -> User {
        return try await makeRequest(endpoint: "/api/v1/user/profile")
    }

    // MARK: - Chat Endpoints

    func getChatHistory() async throws -> [Message] {
        return try await makeRequest(endpoint: "/api/v1/chat/history")
    }
}
