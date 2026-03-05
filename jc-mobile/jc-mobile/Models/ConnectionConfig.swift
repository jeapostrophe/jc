import Foundation

struct ConnectionConfig: Codable, Equatable {
    let host: String
    let port: Int
    let token: String
    let fingerprint: String

    var wsURL: URL {
        URL(string: "wss://\(host):\(port)")!
    }

    // MARK: - UserDefaults Persistence

    private static let key = "connectionConfig"

    static func load() -> ConnectionConfig? {
        guard let data = UserDefaults.standard.data(forKey: key) else { return nil }
        return try? JSONDecoder().decode(ConnectionConfig.self, from: data)
    }

    func save() {
        if let data = try? JSONEncoder().encode(self) {
            UserDefaults.standard.set(data, forKey: Self.key)
        }
    }

    static func clear() {
        UserDefaults.standard.removeObject(forKey: key)
    }
}
