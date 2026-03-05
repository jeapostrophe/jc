import AVFoundation
import SwiftUI

struct QRScannerView: View {
    let onScan: (ConnectionConfig) -> Void
    var onCancel: (() -> Void)? = nil

    @State private var errorMessage: String?
    @State private var showError = false

    var body: some View {
        ZStack {
            CameraPreview(onQRCode: handleQRCode)
                .ignoresSafeArea()

            // Viewfinder frame
            RoundedRectangle(cornerRadius: 16)
                .strokeBorder(.white.opacity(0.7), lineWidth: 2)
                .frame(width: 240, height: 240)

            VStack {
                Spacer()

                Text("Scan QR code from jc desktop")
                    .font(.subheadline)
                    .fontWeight(.medium)
                    .foregroundStyle(.white)
                    .padding(.horizontal, 20)
                    .padding(.vertical, 10)
                    .background(.black.opacity(0.6), in: Capsule())
                    .padding(.bottom, 24)

                if let onCancel {
                    Button("Cancel", action: onCancel)
                        .font(.body.weight(.medium))
                        .foregroundStyle(.white)
                        .padding(.horizontal, 24)
                        .padding(.vertical, 10)
                        .background(.ultraThinMaterial, in: Capsule())
                        .padding(.bottom, 40)
                }
            }

            // Error overlay
            if showError, let errorMessage {
                Text(errorMessage)
                    .font(.caption)
                    .foregroundStyle(.white)
                    .padding(.horizontal, 16)
                    .padding(.vertical, 8)
                    .background(.red.opacity(0.85), in: Capsule())
                    .transition(.opacity.combined(with: .scale))
                    .offset(y: -60)
            }
        }
    }

    private func handleQRCode(_ code: String) {
        guard let data = code.data(using: .utf8) else {
            flashError("Invalid QR data")
            return
        }
        do {
            let config = try JSONDecoder().decode(ConnectionConfig.self, from: data)
            onScan(config)
        } catch {
            flashError("Not a valid jc QR code")
        }
    }

    private func flashError(_ message: String) {
        errorMessage = message
        withAnimation { showError = true }
        DispatchQueue.main.asyncAfter(deadline: .now() + 2) {
            withAnimation { showError = false }
        }
    }
}

// MARK: - Camera Preview (UIViewRepresentable)

private struct CameraPreview: UIViewRepresentable {
    let onQRCode: (String) -> Void

    func makeCoordinator() -> Coordinator {
        Coordinator(onQRCode: onQRCode)
    }

    func makeUIView(context: Context) -> PreviewUIView {
        let view = PreviewUIView()
        context.coordinator.setup(in: view)
        return view
    }

    func updateUIView(_: PreviewUIView, context _: Context) {}

    class Coordinator: NSObject, AVCaptureMetadataOutputObjectsDelegate {
        let onQRCode: (String) -> Void
        private let session = AVCaptureSession()
        private var hasScanned = false

        init(onQRCode: @escaping (String) -> Void) {
            self.onQRCode = onQRCode
        }

        func setup(in view: PreviewUIView) {
            AVCaptureDevice.requestAccess(for: .video) { granted in
                guard granted else { return }
                DispatchQueue.main.async {
                    self.configureSession(in: view)
                }
            }
        }

        private func configureSession(in view: PreviewUIView) {
            guard let device = AVCaptureDevice.default(for: .video),
                  let input = try? AVCaptureDeviceInput(device: device)
            else { return }

            if session.canAddInput(input) {
                session.addInput(input)
            }

            let output = AVCaptureMetadataOutput()
            if session.canAddOutput(output) {
                session.addOutput(output)
                output.setMetadataObjectsDelegate(self, queue: .main)
                output.metadataObjectTypes = [.qr]
            }

            let previewLayer = AVCaptureVideoPreviewLayer(session: session)
            previewLayer.videoGravity = .resizeAspectFill
            view.previewLayer = previewLayer
            view.layer.addSublayer(previewLayer)

            DispatchQueue.global(qos: .userInitiated).async {
                self.session.startRunning()
            }
        }

        func metadataOutput(
            _: AVCaptureMetadataOutput,
            didOutput metadataObjects: [AVMetadataObject],
            from _: AVCaptureConnection
        ) {
            guard !hasScanned,
                  let object = metadataObjects.first as? AVMetadataMachineReadableCodeObject,
                  let value = object.stringValue
            else { return }

            hasScanned = true
            onQRCode(value)

            // Allow re-scanning after brief delay (in case parse fails)
            DispatchQueue.main.asyncAfter(deadline: .now() + 2.5) {
                self.hasScanned = false
            }
        }
    }
}

/// A plain UIView that hosts an AVCaptureVideoPreviewLayer and keeps it sized.
private class PreviewUIView: UIView {
    var previewLayer: AVCaptureVideoPreviewLayer?

    override func layoutSubviews() {
        super.layoutSubviews()
        previewLayer?.frame = bounds
    }
}

#Preview {
    QRScannerView(onScan: { config in
        print("Scanned: \(config)")
    }, onCancel: {
        print("Cancelled")
    })
}
