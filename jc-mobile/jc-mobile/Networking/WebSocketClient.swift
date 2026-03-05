import CryptoKit
import Foundation
import Network
import os

private let log = Logger(subsystem: "dev.jc", category: "WebSocket")

// MARK: - Errors

enum WebSocketError: Error, LocalizedError {
    case notConnected
    case connectionClosed
    case fingerprintMismatch(expected: String, got: String)
    case decodingFailed(String)

    var errorDescription: String? {
        switch self {
        case .notConnected:
            "WebSocket is not connected"
        case .connectionClosed:
            "WebSocket connection was closed"
        case let .fingerprintMismatch(expected, got):
            "Certificate fingerprint mismatch: expected \(expected), got \(got)"
        case let .decodingFailed(detail):
            "Failed to decode server message: \(detail)"
        }
    }
}

// MARK: - WebSocketClient (NWConnection + NWProtocolWebSocket)

class WebSocketClient {
    private var connection: NWConnection?
    private let expectedFingerprint: String
    private let encoder = JSONEncoder()
    private let decoder = JSONDecoder()
    private let queue = DispatchQueue(label: "dev.jc.websocket")

    init(fingerprint: String) {
        self.expectedFingerprint = fingerprint
    }

    // MARK: - Connection

    func connect(to url: URL) async throws {
        guard let host = url.host, let port = url.port else {
            throw WebSocketError.notConnected
        }

        log.info("Connecting to \(host):\(port) (fingerprint: \(self.expectedFingerprint.prefix(11))...)")

        // TLS with custom cert verification (pinning via SHA-256 fingerprint)
        let tlsOptions = NWProtocolTLS.Options()
        let expectedFP = expectedFingerprint

        sec_protocol_options_set_verify_block(
            tlsOptions.securityProtocolOptions,
            { _, trust, complete in
                log.info("TLS verify block called")

                let secTrust = sec_trust_copy_ref(trust).takeRetainedValue()
                guard SecTrustGetCertificateCount(secTrust) > 0,
                      let chain = SecTrustCopyCertificateChain(secTrust),
                      let leaf = (chain as! [SecCertificate]).first
                else {
                    log.error("No server certificate in chain")
                    complete(false)
                    return
                }

                let der = SecCertificateCopyData(leaf) as Data
                let fp = SHA256.hash(data: der)
                    .map { String(format: "%02X", $0) }
                    .joined(separator: ":")

                if fp == expectedFP {
                    log.info("Cert pinning OK")
                    complete(true)
                } else {
                    log.error("Fingerprint mismatch — expected: \(expectedFP) got: \(fp)")
                    complete(false)
                }
            },
            queue
        )

        // WebSocket protocol on top of TLS
        let wsOptions = NWProtocolWebSocket.Options()
        let params = NWParameters(tls: tlsOptions)
        params.defaultProtocolStack.applicationProtocols.insert(wsOptions, at: 0)

        let conn = NWConnection(
            host: .init(host),
            port: .init(rawValue: UInt16(port))!,
            using: params
        )
        connection = conn

        // Wait for .ready state
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Void, Error>) in
            var resumed = false
            conn.stateUpdateHandler = { state in
                guard !resumed else { return }
                switch state {
                case .ready:
                    resumed = true
                    log.info("Connection ready")
                    conn.stateUpdateHandler = nil
                    cont.resume()
                case let .failed(error):
                    resumed = true
                    log.error("Connection failed: \(error)")
                    cont.resume(throwing: error)
                case .cancelled:
                    resumed = true
                    cont.resume(throwing: WebSocketError.connectionClosed)
                case let .waiting(error):
                    log.info("Connection waiting: \(error)")
                default:
                    break
                }
            }
            conn.start(queue: self.queue)
        }
    }

    // MARK: - Send

    func send(_ message: AuthMessage) async throws {
        guard let connection else {
            throw WebSocketError.notConnected
        }

        let data = try encoder.encode(message)
        let meta = NWProtocolWebSocket.Metadata(opcode: .text)
        let ctx = NWConnection.ContentContext(identifier: "ws", metadata: [meta])

        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Void, Error>) in
            connection.send(
                content: data,
                contentContext: ctx,
                isComplete: true,
                completion: .contentProcessed { error in
                    if let error {
                        cont.resume(throwing: error)
                    } else {
                        cont.resume()
                    }
                }
            )
        }
    }

    // MARK: - Receive

    func receive() async throws -> ServerMessage {
        guard let connection else {
            throw WebSocketError.notConnected
        }

        let data: Data = try await withCheckedThrowingContinuation { cont in
            connection.receiveMessage { content, _, _, error in
                if let error {
                    cont.resume(throwing: error)
                } else if let content {
                    cont.resume(returning: content)
                } else {
                    cont.resume(throwing: WebSocketError.connectionClosed)
                }
            }
        }

        do {
            return try decoder.decode(ServerMessage.self, from: data)
        } catch {
            let preview = String(data: data.prefix(200), encoding: .utf8) ?? "<binary>"
            log.error("Decode failed: \(error.localizedDescription) — \(preview)")
            throw WebSocketError.decodingFailed(error.localizedDescription)
        }
    }

    // MARK: - Disconnect

    func disconnect() {
        connection?.cancel()
        connection = nil
    }
}
