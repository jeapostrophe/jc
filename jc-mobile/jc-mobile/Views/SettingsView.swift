import SwiftUI

struct SettingsView: View {
    let config: ConnectionConfig?
    let isConnected: Bool
    var onUnpair: () -> Void = {}
    var onRescan: () -> Void = {}

    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            List {
                // Connection status
                Section("Connection") {
                    HStack {
                        Text("Status")
                        Spacer()
                        HStack(spacing: 6) {
                            Circle()
                                .fill(isConnected ? .green : .red)
                                .frame(width: 8, height: 8)
                            Text(isConnected ? "Connected" : "Disconnected")
                                .foregroundStyle(isConnected ? .green : .red)
                        }
                        .font(.subheadline)
                    }

                    if let config {
                        HStack {
                            Text("Host")
                            Spacer()
                            Text("\(config.host):\(config.port)")
                                .foregroundStyle(.secondary)
                                .font(.subheadline.monospaced())
                        }
                    }
                }

                // Actions
                Section {
                    Button(action: onRescan) {
                        Label("Re-scan QR Code", systemImage: "qrcode.viewfinder")
                    }

                    Button(role: .destructive, action: onUnpair) {
                        Label("Unpair", systemImage: "link.badge.plus")
                            .symbolRenderingMode(.multicolor)
                    }
                }
            }
            .listStyle(.insetGrouped)
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Done") { dismiss() }
                }
            }
        }
    }
}

#Preview("Connected") {
    SettingsView(
        config: ConnectionConfig(
            host: "192.168.1.42", port: 9120,
            token: "abc123", fingerprint: "deadbeef"),
        isConnected: true)
}

#Preview("Disconnected") {
    SettingsView(config: nil, isConnected: false)
}
