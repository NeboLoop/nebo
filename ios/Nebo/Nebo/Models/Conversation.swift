import Foundation

struct Conversation: Codable, Identifiable {
    let id: String
    var messages: [Message]
    let createdAt: Date
    var updatedAt: Date

    init(id: String = UUID().uuidString, messages: [Message] = [], createdAt: Date = Date(), updatedAt: Date = Date()) {
        self.id = id
        self.messages = messages
        self.createdAt = createdAt
        self.updatedAt = updatedAt
    }
}
