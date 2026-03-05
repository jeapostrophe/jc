import SwiftUI

struct ProjectSection: View {
    let project: MobileProject
    let isActive: Bool

    var body: some View {
        Section {
            ForEach(Array(project.sessions.enumerated()), id: \.element.id) { index, session in
                SessionRow(
                    session: session,
                    isActive: isActive && project.activeSessionIndex == index)
            }
        } header: {
            HStack(spacing: 6) {
                Text(project.name)
                    .fontWeight(isActive ? .semibold : .regular)
                    .foregroundStyle(isActive ? .primary : .secondary)

                Spacer()

                if !project.problems.isEmpty {
                    Text("\(project.problems.count)")
                        .font(.caption2.weight(.bold))
                        .foregroundStyle(.white)
                        .padding(.horizontal, 6)
                        .padding(.vertical, 2)
                        .background(.orange, in: Capsule())
                }
            }
        }
    }
}

#Preview {
    let mock = MobileStateSnapshot.mock
    List {
        ProjectSection(project: mock.projects[0], isActive: true)
        ProjectSection(project: mock.projects[1], isActive: false)
    }
}
