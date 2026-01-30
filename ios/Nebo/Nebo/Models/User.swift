import Foundation

struct User: Codable, Identifiable {
    let id: String
    let email: String
    let name: String?
}

struct AuthTokens: Codable {
    let accessToken: String
    let refreshToken: String?
    let expiresAt: Date?
}

struct LoginRequest: Codable {
    let email: String
    let password: String
}

struct LoginResponse: Codable {
    let user: User
    let accessToken: String
    let refreshToken: String?
    let expiresIn: Int?
}
