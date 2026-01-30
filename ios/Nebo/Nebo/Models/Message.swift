import Foundation

enum MessageRole: String, Codable {
    case user
    case assistant
}

enum MessageStatus: String, Codable {
    case sending
    case sent
    case failed
}

struct Message: Codable, Identifiable, Equatable {
    let id: String
    let role: MessageRole
    var content: String
    let timestamp: Date
    var status: MessageStatus

    init(id: String = UUID().uuidString, role: MessageRole, content: String, timestamp: Date = Date(), status: MessageStatus = .sending) {
        self.id = id
        self.role = role
        self.content = content
        self.timestamp = timestamp
        self.status = status
    }
}
