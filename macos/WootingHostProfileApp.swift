import AppKit
import Darwin
import SwiftUI

private final class AgentOutput: @unchecked Sendable {
    private let lock = NSLock()
    private var storage = Data()

    func store(_ data: Data) {
        lock.lock()
        storage = data
        lock.unlock()
    }

    var data: Data {
        lock.lock()
        defer { lock.unlock() }
        return storage
    }
}

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
    @Published var linkedProfileCount: Int?
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

    var selectedProfileInfo: KeyboardProfile? {
        profiles.first(where: { $0.index == selectedProfile })
    }

    var rememberTitle: String {
        guard let profile = selectedProfileInfo else { return "Remember & activate" }
        if profile.active && profile.assigned == true { return "Up to date" }
        if profile.assigned == true { return "Activate" }
        return "Remember & activate"
    }

    var canRemember: Bool {
        guard let profile = selectedProfileInfo else { return false }
        return !busy && !(profile.active && profile.assigned == true)
    }

    var appLinkingStatus: String {
        guard let count = linkedProfileCount else {
            return "Wootility App Linking status is unavailable."
        }
        return count == 0
            ? "Wootility App Linking: no linked profiles detected."
            : "Conflict detected: \(count) Wootility linked profile(s) can override this app."
    }

    var appLinkingHasConflict: Bool {
        (linkedProfileCount ?? 0) > 0
    }

    var appLinkingStatusIcon: String {
        guard linkedProfileCount != nil else { return "info.circle" }
        return appLinkingHasConflict ? "exclamationmark.triangle" : "checkmark.circle"
    }

    func runAgent(_ arguments: [String]) async throws -> String {
        let executableURL = agentURL
        return try await Task.detached(priority: .userInitiated) {
            try Self.runAgentBlocking(executableURL, arguments: arguments)
        }.value
    }

    nonisolated private static func runAgentBlocking(_ executableURL: URL, arguments: [String]) throws -> String {
        let process = Process()
        let output = Pipe()
        let errors = Pipe()
        let stdout = AgentOutput()
        let stderr = AgentOutput()
        let readers = DispatchGroup()
        let exited = DispatchSemaphore(value: 0)
        process.executableURL = executableURL
        process.arguments = arguments
        process.standardOutput = output
        process.standardError = errors

        process.terminationHandler = { _ in exited.signal() }
        try process.run()

        // Drain both pipes concurrently. If either pipe fills, the child can
        // block before it exits; reading them serially would deadlock.
        readers.enter()
        DispatchQueue.global(qos: .utility).async {
            stdout.store(output.fileHandleForReading.readDataToEndOfFile())
            readers.leave()
        }
        readers.enter()
        DispatchQueue.global(qos: .utility).async {
            stderr.store(errors.fileHandleForReading.readDataToEndOfFile())
            readers.leave()
        }

        let timeout: TimeInterval = 30
        guard exited.wait(timeout: .now() + timeout) == .success else {
            process.terminate()
            // Give the child a short grace period, then force termination if it
            // ignored SIGTERM. The second wait is bounded as well.
            if exited.wait(timeout: .now() + 2) != .success {
                _ = Darwin.kill(process.processIdentifier, SIGKILL)
                _ = exited.wait(timeout: .now() + 2)
            }
            try? output.fileHandleForReading.close()
            try? errors.fileHandleForReading.close()
            throw NSError(
                domain: "WootingSwitch",
                code: Int(ETIMEDOUT),
                userInfo: [NSLocalizedDescriptionKey: "The background agent timed out and was terminated."]
            )
        }

        // A descendant may inherit a pipe and keep it open after the agent exits.
        // Bound that wait too, so the UI cannot remain stuck indefinitely.
        guard readers.wait(timeout: .now() + 2) == .success else {
            try? output.fileHandleForReading.close()
            try? errors.fileHandleForReading.close()
            throw NSError(
                domain: "WootingSwitch",
                code: Int(ETIMEDOUT),
                userInfo: [NSLocalizedDescriptionKey: "The background agent left an output pipe open."]
            )
        }
        try? output.fileHandleForReading.close()
        try? errors.fileHandleForReading.close()
        let stdoutText = String(data: stdout.data, encoding: .utf8) ?? ""
        let stderrText = String(data: stderr.data, encoding: .utf8) ?? ""
        guard process.terminationStatus == 0 else {
            throw NSError(
                domain: "WootingSwitch",
                code: Int(process.terminationStatus),
                userInfo: [NSLocalizedDescriptionKey: stderrText.isEmpty ? "The background agent failed." : stderrText]
            )
        }
        return stdoutText.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    func refresh(successMessage: String? = nil) {
        busy = true
        status = ""
        Task {
            do {
                startAtLogin = try await runAgent(["startup-status"]) == "true"
                automaticSwitching = try await runAgent(["enabled-status"]) == "true"
                keepProfileActive = try await runAgent(["enforce-status"]) == "true"
                hideStatusIcon = try await runAgent(["status-icon-status"]) != "true"
                linkedProfileCount = Int(try await runAgent(["linked-status"]))
                applyStatusIconVisibility()

                let json = try await runAgent(["profiles", "--json"])
                let decoded = try JSONDecoder().decode([KeyboardProfile].self, from: Data(json.utf8))
                profiles = decoded
                selectedProfile = decoded.first(where: { $0.assigned == true })?.index
                    ?? decoded.first(where: { $0.active })?.index
                    ?? decoded.first?.index
                    ?? 0
                if decoded.isEmpty {
                    status = "No configured onboard profiles found."
                } else if !decoded.contains(where: { $0.active }) {
                    status = "Keyboard status unavailable. Showing saved profile information."
                } else if let successMessage {
                    status = successMessage
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
        let profile = selectedProfileInfo
        let wasAssigned = profile?.assigned == true
        let profileIndex = selectedProfile
        busy = true
        status = ""
        Task {
            do {
                _ = try await runAgent([
                    "configure", "--profile", String(profileIndex),
                    "--startup", "keep"
                ])
                refresh(successMessage: wasAssigned
                    ? "P\(profileIndex) activated and verified."
                    : "P\(profileIndex) saved for macOS and verified.")
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
                _ = try await runAgent(arguments)
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
                        Text("\(model.profiles.count) / 4 onboard profiles")
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
                                if profile.active && profile.assigned == true {
                                    Text("ACTIVE · MAC DEFAULT")
                                } else if profile.active {
                                    Text("ACTIVE")
                                } else if profile.assigned == true {
                                    Text("MAC DEFAULT")
                                }
                            }
                            .tag(profile.index)
                        }
                    }
                    .labelsHidden()
                    .pickerStyle(.menu)
                    .frame(maxWidth: .infinity, minHeight: 36, maxHeight: 36)

                    Button(model.rememberTitle) { model.remember() }
                        .buttonStyle(.borderedProminent)
                        .tint(accent)
                        .foregroundStyle(Color.black)
                        .frame(height: 36)
                        .disabled(!model.canRemember)
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
                    Image(systemName: model.appLinkingStatusIcon)
                        .font(.system(size: 11))
                        .foregroundStyle(model.appLinkingHasConflict ? accent : muted)
                    Text(model.appLinkingStatus)
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
