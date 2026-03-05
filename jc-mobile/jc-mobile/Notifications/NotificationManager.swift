import Foundation
import UIKit
import UserNotifications

// MARK: - Problem Identity

private struct ProblemIdentity: Hashable {
    let projectName: String
    let sessionSlug: String
    let description: String
}

// MARK: - NotificationManager

class NotificationManager {
    private var previousProblems: Set<ProblemIdentity> = []

    static let sessionProblemCategory = "SESSION_PROBLEM"

    // MARK: - Permission

    func requestPermission() {
        let center = UNUserNotificationCenter.current()

        // Register category for future action buttons.
        let category = UNNotificationCategory(
            identifier: Self.sessionProblemCategory,
            actions: [],
            intentIdentifiers: [],
            options: []
        )
        center.setNotificationCategories([category])

        center.requestAuthorization(options: [.alert, .sound, .badge]) { granted, error in
            if let error {
                print("[Notifications] Permission error: \(error.localizedDescription)")
            } else {
                print("[Notifications] Permission \(granted ? "granted" : "denied")")
            }
        }
    }

    // MARK: - Snapshot Handling

    func handleSnapshot(_ snapshot: MobileStateSnapshot) {
        let currentProblems = extractProblems(from: snapshot)
        let newProblems = currentProblems.subtracting(previousProblems)
        previousProblems = currentProblems

        guard !newProblems.isEmpty else { return }

        // Only fire notifications when the app is backgrounded.
        let isActive = DispatchQueue.main.sync {
            UIApplication.shared.applicationState == .active
        }
        guard !isActive else { return }

        // Look up session labels for notification titles.
        let sessionLabels = buildSessionLabelMap(from: snapshot)

        for problem in newProblems {
            let label = sessionLabels[problem.sessionSlug] ?? problem.sessionSlug
            fireNotification(
                projectName: problem.projectName,
                sessionLabel: label,
                description: problem.description
            )
        }
    }

    // MARK: - Private Helpers

    private func extractProblems(from snapshot: MobileStateSnapshot) -> Set<ProblemIdentity> {
        var result = Set<ProblemIdentity>()
        for project in snapshot.projects {
            for session in project.sessions {
                for problem in session.problems {
                    result.insert(ProblemIdentity(
                        projectName: project.name,
                        sessionSlug: session.slug,
                        description: problem.description
                    ))
                }
            }
        }
        return result
    }

    private func buildSessionLabelMap(from snapshot: MobileStateSnapshot) -> [String: String] {
        var map: [String: String] = [:]
        for project in snapshot.projects {
            for session in project.sessions {
                map[session.slug] = session.label
            }
        }
        return map
    }

    private func fireNotification(projectName: String, sessionLabel: String, description: String) {
        let content = UNMutableNotificationContent()
        content.title = "jc: \(projectName) / \(sessionLabel)"
        content.body = description
        content.sound = .default
        content.categoryIdentifier = Self.sessionProblemCategory

        let id = "\(projectName)-\(sessionLabel)-\(description.hashValue)"
        let request = UNNotificationRequest(
            identifier: id,
            content: content,
            trigger: nil // Deliver immediately.
        )

        UNUserNotificationCenter.current().add(request) { error in
            if let error {
                print("[Notifications] Failed to schedule: \(error.localizedDescription)")
            }
        }
    }
}
