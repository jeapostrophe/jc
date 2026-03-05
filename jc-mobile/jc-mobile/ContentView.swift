import SwiftUI

struct ContentView: View {
    @Environment(ConnectionManager.self) private var connection
    @State private var showSettings = false

    private var isPaired: Bool { connection.config != nil }
    private var isConnected: Bool { connection.state == .connected }

    var body: some View {
        if isPaired {
            DashboardView(
                snapshot: connection.snapshot ?? MobileStateSnapshot(),
                connectionState: connection.state,
                onSettings: { showSettings = true }
            )
            .sheet(isPresented: $showSettings) {
                SettingsView(
                    config: connection.config,
                    isConnected: isConnected,
                    onUnpair: {
                        connection.unpair()
                        showSettings = false
                    },
                    onRescan: {
                        connection.unpair()
                        showSettings = false
                    })
            }
        } else {
            QRScannerView(onScan: { config in
                connection.connect(config: config)
                NotificationManager().requestPermission()
            })
        }
    }
}

#Preview {
    ContentView()
        .environment(ConnectionManager())
}
