import SwiftUI

struct UsageCardView: View {
    let usage: MobileUsage

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            // Progress bars
            ProgressRow(label: "5h window", pct: usage.fiveHourPct)
            ProgressRow(label: "Weekly", pct: usage.limitPct)

            HStack {
                // Par status
                Text(parLabel)
                    .font(.caption.weight(.medium))
                    .foregroundStyle(parColor)

                Spacer()

                // Remaining hours
                if let remaining = usage.remainingHours {
                    Text("~\(remaining, specifier: "%.0f")h remaining")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
        }
        .padding(12)
        .background(.quaternary.opacity(0.5), in: RoundedRectangle(cornerRadius: 10))
    }

    private var parLabel: String {
        switch usage.parStatus {
        case "Under": return "Under par"
        case "Over": return "Over par"
        default: return "On par"
        }
    }

    private var parColor: Color {
        switch usage.parStatus {
        case "Under": return .green
        case "Over": return .red
        default: return .secondary
        }
    }
}

// MARK: - Progress Row

private struct ProgressRow: View {
    let label: String
    let pct: Double

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack {
                Text(label)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                Spacer()
                Text("\(pct, specifier: "%.0f")%")
                    .font(.caption2.monospacedDigit())
                    .foregroundStyle(.secondary)
            }

            GeometryReader { geo in
                ZStack(alignment: .leading) {
                    RoundedRectangle(cornerRadius: 3)
                        .fill(.quaternary)

                    RoundedRectangle(cornerRadius: 3)
                        .fill(barColor)
                        .frame(width: max(0, geo.size.width * min(pct, 100) / 100))
                }
            }
            .frame(height: 6)
        }
    }

    private var barColor: Color {
        if pct >= 90 { return .red }
        if pct >= 70 { return .orange }
        return .blue
    }
}

#Preview {
    UsageCardView(usage: MobileStateSnapshot.mock.usage!)
        .padding()
}
