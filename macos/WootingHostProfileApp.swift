import AppKit
import Darwin
import SwiftUI

struct KeyboardProfile: Codable, Identifiable, Hashable {
    let index: Int
    let name: String
    let active: Bool
    let assigned: Bool?
    var id: Int { index }
}

@MainActor
final class ProfileModel: ObservableObject {
    @Published var profiles: [KeyboardProfile] = []
    @Published var selectedProfile = 0
    @Published var automaticSwitching = true
    @Published var keepProfileActive = false
    @Published var startAtLogin = true
    @Published var hideStatusIcon = false
    @Published var status = ""
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
                domain: "WootingSwitch",
                code: Int(process.terminationStatus),
                userInfo: [NSLocalizedDescriptionKey: stderr.isEmpty ? "The background agent failed." : stderr]
            )
        }
        return stdout.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    func refresh() {
        busy = true
        status = ""
        Task {
            do {
                startAtLogin = try runAgent(["startup-status"]) == "true"
                automaticSwitching = try runAgent(["enabled-status"]) == "true"
                keepProfileActive = try runAgent(["enforce-status"]) == "true"
                hideStatusIcon = try runAgent(["status-icon-status"]) != "true"
                applyStatusIconVisibility()

                let json = try runAgent(["profiles", "--json"])
                let decoded = try JSONDecoder().decode([KeyboardProfile].self, from: Data(json.utf8))
                profiles = decoded
                selectedProfile = decoded.first(where: { $0.assigned == true })?.index
                    ?? decoded.first(where: { $0.active })?.index
                    ?? decoded.first?.index
                    ?? 0
                if decoded.isEmpty {
                    status = "No configured onboard profiles found."
                }
            } catch {
                profiles = []
                status = error.localizedDescription
            }
            busy = false
        }
    }

    func remember() {
        guard selectedProfile > 0 else { return }
        busy = true
        status = ""
        Task {
            do {
                _ = try runAgent([
                    "configure", "--profile", String(selectedProfile),
                    "--startup", "keep"
                ])
                refresh()
            } catch {
                status = error.localizedDescription
                busy = false
            }
        }
    }

    func setAutomaticSwitching(_ enabled: Bool) {
        guard !busy else { return }
        update(["set-enabled", enabled ? "enable" : "disable"])
    }

    func setEnforcement(_ enabled: Bool) {
        guard !busy else { return }
        update(["set-enforce", enabled ? "enable" : "disable"])
    }

    func setStartup(_ enabled: Bool) {
        guard !busy, selectedProfile > 0 else { return }
        update([
            "configure", "--profile", String(selectedProfile),
            "--startup", enabled ? "enable" : "disable"
        ])
    }

    func setStatusIconHidden(_ hidden: Bool) {
        guard !busy else { return }
        update(["set-status-icon", hidden ? "hide" : "show"]) { [weak self] in
            self?.applyStatusIconVisibility()
        }
    }

    private func update(_ arguments: [String], completion: (() -> Void)? = nil) {
        busy = true
        status = ""
        Task {
            do {
                _ = try runAgent(arguments)
                completion?()
            } catch {
                status = error.localizedDescription
            }
            busy = false
        }
    }

    private func applyStatusIconVisibility() {
        NSApp.setActivationPolicy(hideStatusIcon ? .accessory : .regular)
    }
}

struct ContentView: View {
    @StateObject private var model = ProfileModel()

    private let canvas = Color(red: 24 / 255, green: 26 / 255, blue: 27 / 255)
    private let surface = Color(red: 32 / 255, green: 36 / 255, blue: 38 / 255)
    private let border = Color(red: 52 / 255, green: 58 / 255, blue: 61 / 255)
    private let text = Color(red: 232 / 255, green: 235 / 255, blue: 237 / 255)
    private let muted = Color(red: 166 / 255, green: 173 / 255, blue: 181 / 255)
    private let accent = Color(red: 255 / 255, green: 212 / 255, blue: 92 / 255)

    var body: some View {
        ZStack(alignment: .bottom) {
            canvas.ignoresSafeArea()

            VStack(alignment: .leading, spacing: 11) {
                HStack {
                    VStack(alignment: .leading, spacing: 1) {
                        Text("macOS profile")
                            .font(.system(size: 18, weight: .semibold))
                            .foregroundStyle(text)
                        Text("Choose the profile for this Mac")
                            .font(.system(size: 11))
                            .foregroundStyle(muted)
                    }
                    Spacer()
                    if model.busy {
                        ProgressView().controlSize(.small).tint(accent)
                    }
                }

                HStack(spacing: 8) {
                    Picker("", selection: $model.selectedProfile) {
                        ForEach(model.profiles) { profile in
                            HStack {
                                Text("P\(profile.index)")
                                Text(profile.name)
                                if profile.active {
                                    Text("ACTIVE")
                                }
                            }
                            .tag(profile.index)
                        }
                    }
                    .labelsHidden()
                    .pickerStyle(.menu)
                    .frame(maxWidth: .infinity, minHeight: 36, maxHeight: 36)

                    Button("Remember") { model.remember() }
                        .buttonStyle(.borderedProminent)
                        .tint(accent)
                        .foregroundStyle(Color.black)
                        .frame(height: 36)
                        .disabled(model.busy || model.selectedProfile == 0)
                }

                VStack(alignment: .leading, spacing: 4) {
                    Toggle("Switch profiles automatically", isOn: $model.automaticSwitching)
                        .onChange(of: model.automaticSwitching) { model.setAutomaticSwitching($0) }

                    Toggle(isOn: $model.keepProfileActive) {
                        VStack(alignment: .leading, spacing: 0) {
                            Text("Keep this profile active")
                            Text("Checks every 5 seconds")
                                .font(.system(size: 10))
                                .foregroundStyle(muted)
                        }
                    }
                    .onChange(of: model.keepProfileActive) { model.setEnforcement($0) }

                    Toggle("Open at sign-in", isOn: $model.startAtLogin)
                        .onChange(of: model.startAtLogin) { model.setStartup($0) }

                    Toggle("Hide Dock/menu icon", isOn: $model.hideStatusIcon)
                        .onChange(of: model.hideStatusIcon) { model.setStatusIconHidden($0) }
                }
                .toggleStyle(.checkbox)
                .foregroundStyle(text)
                .padding(.horizontal, 10)
                .padding(.vertical, 7)
                .background(surface, in: RoundedRectangle(cornerRadius: 10))
                .overlay(RoundedRectangle(cornerRadius: 10).stroke(border, lineWidth: 1))

                HStack(alignment: .top, spacing: 7) {
                    Image(systemName: "exclamationmark.triangle")
                        .font(.system(size: 11))
                        .foregroundStyle(accent)
                    Text("Wooting’s app-profile syncing may conflict with this app.")
                        .font(.system(size: 10))
                        .foregroundStyle(muted)
                }
            }
            .padding(.horizontal, 16)
            .padding(.top, 10)
            .padding(.bottom, 12)

            if !model.status.isEmpty {
                Text(model.status)
                    .font(.system(size: 11))
                    .foregroundStyle(text)
                    .lineLimit(2)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 8)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(Color(red: 48 / 255, green: 53 / 255, blue: 56 / 255))
                    .overlay(RoundedRectangle(cornerRadius: 8).stroke(accent, lineWidth: 1))
                    .clipShape(RoundedRectangle(cornerRadius: 8))
                    .padding(16)
            }
        }
        .frame(width: 460)
        .fixedSize(horizontal: false, vertical: true)
        .onAppear { model.refresh() }
    }
}

@main
struct WootingSwitchApp: App {
    init() {
        let bundleIdentifier = "io.local.wooting-host-profile"
        let currentProcess = ProcessInfo.processInfo.processIdentifier
        if let existing = NSRunningApplication.runningApplications(withBundleIdentifier: bundleIdentifier)
            .first(where: { $0.processIdentifier != currentProcess }) {
            existing.activate(options: [.activateAllWindows, .activateIgnoringOtherApps])
            exit(0)
        }
    }

    var body: some Scene {
        WindowGroup("Wooting Switch") {
            ContentView()
        }
        .windowResizability(.contentSize)
    }
}
