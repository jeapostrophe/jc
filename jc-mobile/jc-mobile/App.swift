import SwiftUI

@main
struct JCMobileApp: App {
    @State private var connection = ConnectionManager()
    private let notifications = NotificationManager()

    var body: some Scene {
        WindowGroup {
            ContentView()
                .environment(connection)
                .onChange(of: connection.snapshot) { _, newSnapshot in
                    if let newSnapshot {
                        notifications.handleSnapshot(newSnapshot)
                    }
                }
        }
    }
}
