import Foundation
import Network
import UIKit
import os

private let log = Logger(subsystem: "dev.jc", category: "Connection")

@Observable
class ConnectionManager {
    enum State: Equatable {
        case disconnected
        case connecting
        case authenticating
        case connected
        case failed(String)
    }

    // MARK: - Public state

    var state: State = .disconnected
    var snapshot: MobileStateSnapshot?
    var config: ConnectionConfig?

    // MARK: - Private

    private var client: WebSocketClient?
    private var connectionTask: Task<Void, Never>?
    private var pathMonitor: NWPathMonitor?
    private var monitorQueue = DispatchQueue(label: "dev.jc.network-monitor")
    private var retryCount = 0
    private var foregroundObserver: NSObjectProtocol?

    // MARK: - Init / Deinit

    init() {
        config = ConnectionConfig.load()
        startNetworkMonitor()
        observeForeground()
        // Auto-connect if we have a saved config from a previous session.
        if config != nil {
            startConnection()
        }
    }

    deinit {
        stopNetworkMonitor()
        if let foregroundObserver {
            NotificationCenter.default.removeObserver(foregroundObserver)
        }
        connectionTask?.cancel()
    }

    // MARK: - Public API

    func connect(config: ConnectionConfig) {
        self.config = config
        config.save()
        startConnection()
    }

    func disconnect() {
        connectionTask?.cancel()
        connectionTask = nil
        client?.disconnect()
        client = nil
        state = .disconnected
    }

    func unpair() {
        disconnect()
        snapshot = nil
        config = nil
        ConnectionConfig.clear()
    }

    // MARK: - Connection lifecycle

    private func startConnection() {
        // Cancel any existing attempt.
        connectionTask?.cancel()
        connectionTask = nil
        client?.disconnect()
        client = nil

        guard let config else {
            state = .failed("No connection config")
            return
        }

        state = .connecting

        connectionTask = Task { [weak self] in
            guard let self else { return }
            do {
                try await performConnection(config: config)
            } catch is CancellationError {
                // Intentional disconnect -- do nothing.
            } catch {
                guard !Task.isCancelled else { return }
                let message = "\(type(of: error)): \(error.localizedDescription.isEmpty ? String(describing: error) : error.localizedDescription)"
                log.error("Connection failed: \(message)")
                state = .failed(message)
                scheduleReconnect()
            }
        }
    }

    private func performConnection(config: ConnectionConfig) async throws {
        let ws = WebSocketClient(fingerprint: config.fingerprint)
        client = ws

        // 1. Connect
        try Task.checkCancellation()
        try await ws.connect(to: config.wsURL)

        // 2. Wait for AuthChallenge
        try Task.checkCancellation()
        state = .authenticating

        let challenge = try await ws.receive()
        guard case .authChallenge = challenge else {
            throw ConnectionError.unexpectedMessage("Expected AuthChallenge, got other")
        }

        // 3. Send Auth with the token from the QR code
        try Task.checkCancellation()
        try await ws.send(AuthMessage(token: config.token))

        // 4. Wait for AuthResult
        let result = try await ws.receive()
        guard case let .authResult(success) = result else {
            throw ConnectionError.unexpectedMessage("Expected AuthResult, got other")
        }
        guard success else {
            throw ConnectionError.authRejected
        }

        // 5. Authenticated -- start receiving state.
        retryCount = 0
        state = .connected
        log.info("Authenticated and connected")

        try await receiveLoop(ws: ws)
    }

    private func receiveLoop(ws: WebSocketClient) async throws {
        while !Task.isCancelled {
            let message = try await ws.receive()
            switch message {
            case let .stateSnapshot(newSnapshot):
                snapshot = newSnapshot
            case .authChallenge, .authResult:
                log.warning("Unexpected message post-auth: \(String(describing: message))")
            }
        }
    }

    // MARK: - Reconnect

    private func scheduleReconnect() {
        guard !Task.isCancelled else { return }

        let delay = backoffDelay()
        retryCount += 1

        log.info("Reconnecting in \(delay)s (attempt \(self.retryCount))")

        connectionTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(delay))
            guard let self, !Task.isCancelled else { return }
            self.startConnection()
        }
    }

    private func backoffDelay() -> Double {
        let base = pow(2.0, Double(retryCount))
        return min(base, 8.0)
    }

    // MARK: - Network Monitor

    private func startNetworkMonitor() {
        let monitor = NWPathMonitor()
        monitor.pathUpdateHandler = { [weak self] path in
            guard let self else { return }
            if path.status == .satisfied {
                Task { @MainActor in
                    if case .disconnected = self.state, self.config != nil {
                        log.info("Network available — reconnecting")
                        self.startConnection()
                    } else if case .failed = self.state, self.config != nil {
                        log.info("Network available — reconnecting after failure")
                        self.startConnection()
                    }
                }
            }
        }
        monitor.start(queue: monitorQueue)
        pathMonitor = monitor
    }

    private func stopNetworkMonitor() {
        pathMonitor?.cancel()
        pathMonitor = nil
    }

    // MARK: - Foreground Observer

    private func observeForeground() {
        foregroundObserver = NotificationCenter.default.addObserver(
            forName: UIScene.willEnterForegroundNotification,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            guard let self else { return }
            switch state {
            case .disconnected, .failed:
                if config != nil {
                    log.info("Entering foreground — reconnecting")
                    startConnection()
                }
            case .connecting, .authenticating, .connected:
                break
            }
        }
    }
}

// MARK: - Errors

private enum ConnectionError: Error, LocalizedError {
    case unexpectedMessage(String)
    case authRejected

    var errorDescription: String? {
        switch self {
        case let .unexpectedMessage(detail):
            "Unexpected server message: \(detail)"
        case .authRejected:
            "Authentication rejected by server"
        }
    }
}
