import SwiftUI

struct DashboardView: View {
    let snapshot: MobileStateSnapshot
    let connectionState: ConnectionManager.State
    var onSettings: () -> Void = {}

    var body: some View {
        NavigationStack {
            List {
                // Connection status (when not connected)
                if connectionState != .connected {
                    Section {
                        HStack(spacing: 8) {
                            connectionIndicator
                            Text(connectionLabel)
                                .font(.subheadline)
                                .foregroundStyle(connectionColor)
                            if case .connecting = connectionState {
                                Spacer()
                                ProgressView()
                                    .controlSize(.small)
                            } else if case .authenticating = connectionState {
                                Spacer()
                                ProgressView()
                                    .controlSize(.small)
                            }
                        }
                    }
                }

                // Usage card
                if let usage = snapshot.usage {
                    Section {
                        UsageCardView(usage: usage)
                            .listRowInsets(EdgeInsets(top: 4, leading: 16, bottom: 4, trailing: 16))
                    }
                }

                // Projects
                if snapshot.projects.isEmpty {
                    Section {
                        Text("No projects open")
                            .foregroundStyle(.secondary)
                            .frame(maxWidth: .infinity, alignment: .center)
                            .padding(.vertical, 24)
                    }
                } else {
                    ForEach(Array(snapshot.projects.enumerated()), id: \.element.id) { index, project in
                        ProjectSection(
                            project: project,
                            isActive: index == snapshot.activeProjectIndex)
                    }
                }
            }
            .listStyle(.insetGrouped)
            .navigationTitle("jc")
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    connectionDot
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button(action: onSettings) {
                        Image(systemName: "gearshape")
                    }
                }
            }
        }
    }

    // MARK: - Connection indicators

    private var connectionDot: some View {
        Circle()
            .fill(connectionColor)
            .frame(width: 8, height: 8)
    }

    private var connectionIndicator: some View {
        Circle()
            .fill(connectionColor)
            .frame(width: 8, height: 8)
    }

    private var connectionColor: Color {
        switch connectionState {
        case .connected: .green
        case .connecting, .authenticating: .orange
        case .disconnected: .red
        case .failed: .red
        }
    }

    private var connectionLabel: String {
        switch connectionState {
        case .connected: "Connected"
        case .connecting: "Connecting..."
        case .authenticating: "Authenticating..."
        case .disconnected: "Disconnected"
        case let .failed(msg): "Failed: \(msg)"
        }
    }
}

#Preview {
    DashboardView(snapshot: .mock, connectionState: .connected)
}
