import SwiftUI

struct KeyboardProfile: Codable, Identifiable, Hashable {
    let index: Int
    let name: String
    let active: Bool
    var id: Int { index }
}

@MainActor
final class ProfileModel: ObservableObject {
    @Published var profiles: [KeyboardProfile] = []
    @Published var selectedProfile: Int = 0
    @Published var startAtLogin = true
    @Published var status = "Reading Wootility profiles…"
    @Published var busy = false

    private var agentURL: URL {
        Bundle.main.resourceURL!.appendingPathComponent("wooting-host-profile-agent")
    }

    func runAgent(_ arguments: [String]) throws -> String {
        let process = Process()
        let output = Pipe()
        let errors = Pipe()
        process.executableURL = agentURL
        process.arguments = arguments
        process.standardOutput = output
        process.standardError = errors
        try process.run()
        process.waitUntilExit()
        let stdout = String(data: output.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
        let stderr = String(data: errors.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
        guard process.terminationStatus == 0 else {
            throw NSError(
                domain: "WootingHostProfile",
                code: Int(process.terminationStatus),
                userInfo: [NSLocalizedDescriptionKey: stderr.isEmpty ? "The background agent failed." : stderr]
            )
        }
        return stdout
    }

    func refresh() {
        busy = true
        status = "Reading configured profiles…"
        Task {
            do {
                let startup = try runAgent(["startup-status"])
                    .trimmingCharacters(in: .whitespacesAndNewlines)
                startAtLogin = startup == "true"
                let json = try runAgent(["profiles", "--json"])
                let decoded = try JSONDecoder().decode([KeyboardProfile].self, from: Data(json.utf8))
                profiles = decoded
                if let active = decoded.first(where: { $0.active }) {
                    selectedProfile = active.index
                } else if let first = decoded.first {
                    selectedProfile = first.index
                }
                status = decoded.isEmpty ? "No configured onboard profiles found." : "Choose the macOS profile."
            } catch {
                profiles = []
                status = error.localizedDescription
            }
            busy = false
        }
    }

    func applyNow() {
        guard selectedProfile > 0 else { return }
        busy = true
        Task {
            do {
                _ = try runAgent(["set", "--profile", String(selectedProfile)])
                status = "Applied and verified P\(selectedProfile)."
                refresh()
            } catch {
                status = error.localizedDescription
                busy = false
            }
        }
    }

    func saveAndRun() {
        guard selectedProfile > 0 else { return }
        busy = true
        Task {
            do {
                let startup = startAtLogin ? "enable" : "disable"
                _ = try runAgent([
                    "configure", "--profile", String(selectedProfile),
                    "--startup", startup
                ])
                status = "Saved P\(selectedProfile). The hidden watcher is running."
            } catch {
                status = error.localizedDescription
            }
            busy = false
        }
    }
}

struct ContentView: View {
    @StateObject private var model = ProfileModel()

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            VStack(alignment: .leading, spacing: 4) {
                Text("Wooting Host Profile")
                    .font(.title2.bold())
                Text("Choose the configured onboard profile this Mac should use.")
                    .foregroundStyle(.secondary)
            }

            GroupBox("Configured profiles") {
                VStack(alignment: .leading, spacing: 8) {
                    ForEach(model.profiles) { profile in
                        Toggle(isOn: Binding(
                            get: { model.selectedProfile == profile.index },
                            set: { if $0 { model.selectedProfile = profile.index } }
                        )) {
                            HStack {
                                Text("P\(profile.index) — \(profile.name)")
                                if profile.active {
                                    Text("Active now")
                                        .font(.caption)
                                        .foregroundStyle(.secondary)
                                }
                            }
                        }
                        .toggleStyle(.radio)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.vertical, 4)
            }

            HStack(alignment: .top, spacing: 8) {
                Image(systemName: "exclamationmark.triangle.fill")
                    .foregroundStyle(.orange)
                VStack(alignment: .leading, spacing: 3) {
                    Text("Avoid app-specific profile overrides").bold()
                    Text("Do not enable Wootility App Linking or another profile switcher. It can replace the system profile selected here.")
                        .font(.callout)
                        .foregroundStyle(.secondary)
                }
            }
            .padding(10)
            .background(.orange.opacity(0.10), in: RoundedRectangle(cornerRadius: 8))

            Toggle("Start the watcher hidden when I sign in", isOn: $model.startAtLogin)

            HStack {
                Button("Refresh") { model.refresh() }
                Button("Apply Now") { model.applyNow() }
                    .disabled(model.busy || model.selectedProfile == 0)
                Spacer()
                Button("Save and Run in Background") { model.saveAndRun() }
                    .buttonStyle(.borderedProminent)
                    .disabled(model.busy || model.selectedProfile == 0)
            }

            Text(model.status)
                .font(.callout)
                .foregroundStyle(.secondary)
                .lineLimit(2)
        }
        .padding(16)
        .frame(width: 480, height: 380)
        .onAppear { model.refresh() }
    }
}

@main
struct WootingHostProfileApp: App {
    var body: some Scene {
        WindowGroup {
            ContentView()
        }
        .windowResizability(.contentSize)
    }
}
