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
        StatusItemController.shared.setVisible(true)
        NSApp.setActivationPolicy(hideStatusIcon ? .accessory : .regular)
    }
}

private enum WootingTheme {
    static let canvas = Color(red: 24 / 255, green: 26 / 255, blue: 27 / 255)
    static let surface = Color(red: 32 / 255, green: 36 / 255, blue: 38 / 255)
    static let hover = Color(red: 41 / 255, green: 46 / 255, blue: 49 / 255)
    static let border = Color(red: 52 / 255, green: 58 / 255, blue: 61 / 255)
    static let text = Color(red: 232 / 255, green: 235 / 255, blue: 237 / 255)
    static let muted = Color(red: 166 / 255, green: 173 / 255, blue: 181 / 255)
    static let accent = Color(red: 255 / 255, green: 212 / 255, blue: 92 / 255)
    static let accentHover = Color(red: 255 / 255, green: 224 / 255, blue: 138 / 255)
    static let accentPressed = Color(red: 232 / 255, green: 188 / 255, blue: 67 / 255)
    static let status = Color(red: 48 / 255, green: 53 / 255, blue: 56 / 255)
}

@MainActor
private final class StatusItemController: NSObject {
    static let shared = StatusItemController()
    private var item: NSStatusItem?

    func setVisible(_ visible: Bool) {
        if visible, item == nil {
            let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
            if let imageURL = Bundle.main.url(forResource: "AppIcon", withExtension: "icns"),
               let image = NSImage(contentsOf: imageURL) {
                image.size = NSSize(width: 18, height: 18)
                item.button?.image = image
            }
            item.button?.toolTip = "Wooting Switch"
            item.button?.target = self
            item.button?.action = #selector(openWindow)
            item.button?.sendAction(on: [.leftMouseUp, .rightMouseUp])
            self.item = item
        } else if !visible, let item {
            NSStatusBar.system.removeStatusItem(item)
            self.item = nil
        }
    }

    @objc private func openWindow() {
        if NSApp.currentEvent?.type == .rightMouseUp {
            let menu = NSMenu()
            menu.addItem(withTitle: "Open Wooting Switch", action: #selector(showWindow), keyEquivalent: "")
            menu.addItem(.separator())
            menu.addItem(withTitle: "Exit", action: #selector(exitApp), keyEquivalent: "")
            menu.items.forEach { $0.target = self }
            item?.menu = menu
            item?.button?.performClick(nil)
            item?.menu = nil
        } else {
            showWindow()
        }
    }

    @objc func showWindow() {
        NSApp.windows.first?.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    @objc private func exitApp() {
        NSApp.terminate(nil)
    }
}

private struct HoverButton<Label: View>: View {
    let enabled: Bool
    let action: () -> Void
    @ViewBuilder let label: () -> Label
    @State private var hovering = false
    @State private var pressing = false

    var body: some View {
        Button(action: action) { label() }
            .buttonStyle(.plain)
            .background(
                RoundedRectangle(cornerRadius: 7)
                    .fill(background)
            )
            .overlay(
                RoundedRectangle(cornerRadius: 7)
                    .stroke(enabled ? WootingTheme.accent : WootingTheme.border, lineWidth: 1)
            )
            .opacity(enabled ? 1 : 0.48)
            .disabled(!enabled)
            .onHover { hovering = $0 }
            .simultaneousGesture(
                DragGesture(minimumDistance: 0)
                    .onChanged { _ in pressing = true }
                    .onEnded { _ in pressing = false }
            )
    }

    private var background: Color {
        if !enabled { return WootingTheme.surface }
        if pressing { return WootingTheme.accentPressed }
        if hovering { return WootingTheme.accentHover }
        return WootingTheme.accent
    }
}

private struct WootingCheckbox: View {
    let title: String
    var subtitle: String?
    @Binding var checked: Bool
    let enabled: Bool
    @State private var hovering = false

    var body: some View {
        Button {
            guard enabled else { return }
            checked.toggle()
        } label: {
            HStack(spacing: 9) {
                ZStack {
                    RoundedRectangle(cornerRadius: 4)
                        .fill(checked ? WootingTheme.accent : (hovering ? WootingTheme.hover : .clear))
                    RoundedRectangle(cornerRadius: 4)
                        .stroke(checked ? WootingTheme.accent : WootingTheme.muted.opacity(0.7), lineWidth: 1)
                    if checked {
                        Image(systemName: "checkmark")
                            .font(.system(size: 10, weight: .black))
                            .foregroundStyle(WootingTheme.canvas)
                    }
                }
                .frame(width: 17, height: 17)

                VStack(alignment: .leading, spacing: 0) {
                    Text(title)
                        .font(.system(size: 13, weight: .regular))
                        .foregroundStyle(WootingTheme.text)
                    if let subtitle {
                        Text(subtitle)
                            .font(.system(size: 10, weight: .regular))
                            .foregroundStyle(WootingTheme.muted)
                    }
                }
                Spacer(minLength: 0)
            }
            .contentShape(Rectangle())
            .padding(.horizontal, 1)
            .frame(minHeight: subtitle == nil ? 29 : 38)
        }
        .buttonStyle(.plain)
        .disabled(!enabled)
        .opacity(enabled ? 1 : 0.5)
        .onHover { hovering = $0 }
    }
}

private struct ProfileRow: View {
    let profile: KeyboardProfile

    private var state: String {
        if profile.active && profile.assigned == true { return "ACTIVE · MAC DEFAULT" }
        if profile.active { return "ACTIVE" }
        if profile.assigned == true { return "MAC DEFAULT" }
        return ""
    }

    var body: some View {
        HStack(spacing: 9) {
            Text("P\(profile.index)")
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle(WootingTheme.muted)
                .frame(width: 20, alignment: .leading)
            Text(profile.name)
                .font(.system(size: 13))
                .foregroundStyle(WootingTheme.text)
                .lineLimit(1)
            Spacer(minLength: 8)
            if !state.isEmpty {
                Text(state)
                    .font(.system(size: 9, weight: .semibold))
                    .foregroundStyle(WootingTheme.accent)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

private struct ProfileDropdown: View {
    let profiles: [KeyboardProfile]
    @Binding var selection: Int
    let enabled: Bool
    @State private var expanded = false
    @State private var hovering = false

    private var selected: KeyboardProfile? {
        profiles.first(where: { $0.index == selection })
    }

    var body: some View {
        Button {
            if enabled { expanded.toggle() }
        } label: {
            HStack(spacing: 8) {
                if let selected {
                    ProfileRow(profile: selected)
                } else {
                    Text("Choose profile")
                        .font(.system(size: 13))
                        .foregroundStyle(WootingTheme.muted)
                    Spacer()
                }
                Image(systemName: expanded ? "chevron.up" : "chevron.down")
                    .font(.system(size: 9, weight: .bold))
                    .foregroundStyle(WootingTheme.muted)
            }
            .padding(.horizontal, 11)
            .frame(maxWidth: .infinity, minHeight: 36, maxHeight: 36)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .background(hovering ? WootingTheme.hover : WootingTheme.surface)
        .overlay(
            RoundedRectangle(cornerRadius: 7)
                .stroke(expanded ? WootingTheme.accent.opacity(0.8) : WootingTheme.border, lineWidth: 1)
        )
        .clipShape(RoundedRectangle(cornerRadius: 7))
        .opacity(enabled ? 1 : 0.5)
        .onHover { hovering = $0 }
        .overlay(alignment: .topLeading) {
            if expanded {
                VStack(spacing: 2) {
                    ForEach(profiles) { profile in
                        Button {
                            selection = profile.index
                            expanded = false
                        } label: {
                            ProfileRow(profile: profile)
                                .padding(.horizontal, 10)
                                .frame(height: 34)
                                .contentShape(Rectangle())
                        }
                        .buttonStyle(ProfileOptionButtonStyle(selected: profile.index == selection))
                    }
                }
                .padding(4)
                .background(WootingTheme.surface)
                .overlay(RoundedRectangle(cornerRadius: 8).stroke(WootingTheme.border, lineWidth: 1))
                .clipShape(RoundedRectangle(cornerRadius: 8))
                .shadow(color: .black.opacity(0.45), radius: 16, y: 8)
                .offset(y: 40)
                .zIndex(100)
            }
        }
        .zIndex(expanded ? 100 : 1)
    }
}

private struct ProfileOptionButtonStyle: ButtonStyle {
    let selected: Bool

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .background(
                selected
                    ? WootingTheme.hover
                    : configuration.isPressed ? WootingTheme.hover.opacity(0.8) : Color.clear
            )
            .clipShape(RoundedRectangle(cornerRadius: 5))
    }
}

private struct BusyRing: View {
    @State private var spinning = false

    var body: some View {
        Circle()
            .trim(from: 0.12, to: 0.82)
            .stroke(WootingTheme.accent, style: StrokeStyle(lineWidth: 2.2, lineCap: .round))
            .frame(width: 18, height: 18)
            .rotationEffect(.degrees(spinning ? 360 : 0))
            .animation(.linear(duration: 0.8).repeatForever(autoreverses: false), value: spinning)
            .onAppear { spinning = true }
    }
}

struct ContentView: View {
    @StateObject private var model = ProfileModel()

    var body: some View {
        ZStack(alignment: .bottom) {
            WootingTheme.canvas

                VStack(alignment: .leading, spacing: 11) {
                    HStack {
                        VStack(alignment: .leading, spacing: 1) {
                            Text("macOS profile")
                                .font(.system(size: 18, weight: .semibold))
                                .foregroundStyle(WootingTheme.text)
                            Text("\(model.profiles.count) / 4 onboard profiles")
                                .font(.system(size: 11))
                                .foregroundStyle(WootingTheme.muted)
                        }
                        Spacer()
                        if model.busy { BusyRing() }
                    }

                    HStack(spacing: 8) {
                        ProfileDropdown(
                            profiles: model.profiles,
                            selection: $model.selectedProfile,
                            enabled: !model.busy
                        )

                        HoverButton(enabled: model.canRemember, action: model.remember) {
                            Text(model.rememberTitle)
                                .font(.system(size: 13, weight: .semibold))
                                .foregroundStyle(model.canRemember ? WootingTheme.canvas : WootingTheme.muted)
                                .padding(.horizontal, 14)
                                .frame(height: 36)
                        }
                    }
                    .zIndex(100)

                    VStack(spacing: 0) {
                        WootingCheckbox(
                            title: "Switch profiles automatically",
                            checked: automaticSwitching,
                            enabled: !model.busy
                        )
                        WootingCheckbox(
                            title: "Keep this profile active",
                            subtitle: "Checks every 5 seconds",
                            checked: keepProfileActive,
                            enabled: !model.busy
                        )
                        WootingCheckbox(
                            title: "Open at sign-in",
                            checked: startAtLogin,
                            enabled: !model.busy
                        )
                        WootingCheckbox(
                            title: "Hide Dock icon",
                            checked: hideStatusIcon,
                            enabled: !model.busy
                        )
                    }
                    .padding(.horizontal, 10)
                    .padding(.vertical, 7)
                    .background(WootingTheme.surface)
                    .overlay(RoundedRectangle(cornerRadius: 10).stroke(WootingTheme.border, lineWidth: 1))
                    .clipShape(RoundedRectangle(cornerRadius: 10))

                    HStack(alignment: .top, spacing: 7) {
                        Image(systemName: model.appLinkingStatusIcon)
                            .font(.system(size: 11, weight: .semibold))
                            .foregroundStyle(model.appLinkingHasConflict ? WootingTheme.accent : WootingTheme.muted)
                            .padding(.top, 1)
                        Text(model.appLinkingStatus)
                            .font(.system(size: 10))
                            .foregroundStyle(WootingTheme.muted)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
                .padding(.horizontal, 16)
                .padding(.top, 12)
                .padding(.bottom, 14)

                if !model.status.isEmpty {
                    Text(model.status)
                        .font(.system(size: 11))
                        .foregroundStyle(WootingTheme.text)
                        .lineLimit(2)
                        .padding(.horizontal, 10)
                        .padding(.vertical, 8)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .background(WootingTheme.status)
                        .overlay(RoundedRectangle(cornerRadius: 8).stroke(WootingTheme.accent, lineWidth: 1))
                        .clipShape(RoundedRectangle(cornerRadius: 8))
                        .padding(16)
                }
        }
        .frame(width: 460)
        .fixedSize(horizontal: false, vertical: true)
        .background(WootingTheme.canvas)
        .onAppear {
            StatusItemController.shared.setVisible(true)
            model.refresh()
        }
    }

    private var automaticSwitching: Binding<Bool> {
        Binding(
            get: { model.automaticSwitching },
            set: { model.automaticSwitching = $0; model.setAutomaticSwitching($0) }
        )
    }

    private var keepProfileActive: Binding<Bool> {
        Binding(
            get: { model.keepProfileActive },
            set: { model.keepProfileActive = $0; model.setEnforcement($0) }
        )
    }

    private var startAtLogin: Binding<Bool> {
        Binding(
            get: { model.startAtLogin },
            set: { model.startAtLogin = $0; model.setStartup($0) }
        )
    }

    private var hideStatusIcon: Binding<Bool> {
        Binding(
            get: { model.hideStatusIcon },
            set: { model.hideStatusIcon = $0; model.setStatusIconHidden($0) }
        )
    }
}

@MainActor
private final class SingleInstanceController: NSObject {
    static let shared = SingleInstanceController()
    private let showNotification = Notification.Name("io.local.wooting-host-profile.show-window")
    private var lockDescriptor: Int32 = -1

    func claimOrSignalExisting() -> Bool {
        let cacheDirectory = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("WootingHostProfile", isDirectory: true)
        do {
            try FileManager.default.createDirectory(
                at: cacheDirectory,
                withIntermediateDirectories: true
            )
        } catch {
            return fallbackClaim()
        }

        let path = cacheDirectory.appendingPathComponent("app.lock").path
        let descriptor = Darwin.open(path, O_CREAT | O_RDWR, S_IRUSR | S_IWUSR)
        guard descriptor >= 0 else { return fallbackClaim() }

        if Darwin.lockf(descriptor, F_TLOCK, 0) == 0 {
            lockDescriptor = descriptor
            DistributedNotificationCenter.default().addObserver(
                self,
                selector: #selector(showExistingWindow),
                name: showNotification,
                object: nil
            )
            return true
        }

        Darwin.close(descriptor)
        signalExisting()
        return false
    }

    private func fallbackClaim() -> Bool {
        let currentProcess = ProcessInfo.processInfo.processIdentifier
        if NSRunningApplication.runningApplications(withBundleIdentifier: "io.local.wooting-host-profile")
            .contains(where: { $0.processIdentifier != currentProcess }) {
            signalExisting()
            return false
        }
        return true
    }

    private func signalExisting() {
        let currentProcess = ProcessInfo.processInfo.processIdentifier
        DistributedNotificationCenter.default().postNotificationName(
            showNotification,
            object: nil,
            userInfo: nil,
            deliverImmediately: true
        )
        NSRunningApplication.runningApplications(withBundleIdentifier: "io.local.wooting-host-profile")
            .first(where: { $0.processIdentifier != currentProcess })?
            .activate(options: [.activateAllWindows])
    }

    @objc private func showExistingWindow() {
        StatusItemController.shared.showWindow()
    }

    deinit {
        if lockDescriptor >= 0 {
            Darwin.lockf(lockDescriptor, F_ULOCK, 0)
            Darwin.close(lockDescriptor)
        }
    }
}

@main
struct WootingSwitchApp: App {
    init() {
        if !SingleInstanceController.shared.claimOrSignalExisting() {
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
