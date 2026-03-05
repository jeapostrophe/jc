import SwiftUI

struct SessionRow: View {
    let session: MobileSession
    let isActive: Bool

    var body: some View {
        HStack(spacing: 8) {
            // Active indicator
            Circle()
                .fill(isActive ? .green : .clear)
                .frame(width: 8, height: 8)

            Text(session.label)
                .font(.subheadline)
                .lineLimit(1)

            Spacer()

            // Problem count badge
            if !session.problems.isEmpty {
                Text("\(session.problems.count)")
                    .font(.caption2.weight(.bold))
                    .foregroundStyle(.white)
                    .frame(minWidth: 18, minHeight: 18)
                    .background(.red, in: Circle())
            }
        }
        .contentShape(Rectangle())
    }
}

#Preview {
    let mock = MobileStateSnapshot.mock
    List {
        SessionRow(
            session: mock.projects[0].sessions[0],
            isActive: true)
        SessionRow(
            session: mock.projects[0].sessions[1],
            isActive: false)
    }
}
