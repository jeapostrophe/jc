import Foundation

// MARK: - Server -> Client

enum ServerMessage: Decodable {
    case authChallenge(token: String)
    case authResult(success: Bool)
    case stateSnapshot(MobileStateSnapshot)

    private enum CodingKeys: String, CodingKey {
        case type
        case token, success
        case projects, activeProjectIndex, usage
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let type = try container.decode(String.self, forKey: .type)
        switch type {
        case "AuthChallenge":
            let token = try container.decode(String.self, forKey: .token)
            self = .authChallenge(token: token)
        case "AuthResult":
            let success = try container.decode(Bool.self, forKey: .success)
            self = .authResult(success: success)
        case "StateSnapshot":
            let snapshot = try MobileStateSnapshot(from: decoder)
            self = .stateSnapshot(snapshot)
        default:
            throw DecodingError.dataCorruptedError(
                forKey: .type, in: container,
                debugDescription: "Unknown message type: \(type)")
        }
    }
}

// MARK: - Client -> Server

struct AuthMessage: Encodable {
    let type = "Auth"
    let token: String
}

// MARK: - State Snapshot

struct MobileStateSnapshot: Codable, Equatable {
    var projects: [MobileProject] = []
    var activeProjectIndex: Int = 0
    var usage: MobileUsage?

    enum CodingKeys: String, CodingKey {
        case projects
        case activeProjectIndex = "active_project_index"
        case usage
    }
}

struct MobileProject: Codable, Equatable, Identifiable {
    var id: String { name }
    let name: String
    var sessions: [MobileSession] = []
    var activeSessionIndex: Int?
    var problems: [MobileProblem] = []

    enum CodingKeys: String, CodingKey {
        case name, sessions
        case activeSessionIndex = "active_session_index"
        case problems
    }
}

struct MobileSession: Codable, Equatable, Identifiable {
    var id: String { slug }
    let slug: String
    let label: String
    var problems: [MobileProblem] = []
}

struct MobileProblem: Codable, Equatable, Hashable {
    let rank: Int8
    let description: String
}

struct MobileUsage: Codable, Equatable {
    let par: Double
    let parStatus: String
    let limitPct: Double
    let workingPct: Double
    let fiveHourPct: Double
    let pace: Double?
    let remainingHours: Double?

    enum CodingKeys: String, CodingKey {
        case par
        case parStatus = "par_status"
        case limitPct = "limit_pct"
        case workingPct = "working_pct"
        case fiveHourPct = "five_hour_pct"
        case pace
        case remainingHours = "remaining_hours"
    }
}

// MARK: - Mock Data

extension MobileStateSnapshot {
    static let mock = MobileStateSnapshot(
        projects: [
            MobileProject(
                name: "jc",
                sessions: [
                    MobileSession(
                        slug: "encapsulated-swimming-firefly",
                        label: "Refactor auth module",
                        problems: [
                            MobileProblem(rank: 1, description: "Permission prompt"),
                        ]),
                    MobileSession(
                        slug: "vibrant-dancing-otter",
                        label: "Add mobile server",
                        problems: []),
                ],
                activeSessionIndex: 0,
                problems: [
                    MobileProblem(rank: 10, description: "Unreviewed: src/main.rs")
                ]),
            MobileProject(
                name: "other-project",
                sessions: [
                    MobileSession(
                        slug: "quiet-reading-fox",
                        label: "Fix build",
                        problems: [
                            MobileProblem(rank: 3, description: "Claude stopped"),
                        ]),
                ],
                activeSessionIndex: nil,
                problems: []),
        ],
        activeProjectIndex: 0,
        usage: MobileUsage(
            par: 12.5,
            parStatus: "Under",
            limitPct: 38.0,
            workingPct: 50.5,
            fiveHourPct: 22.0,
            pace: 0.75,
            remainingHours: 18.3))
}
